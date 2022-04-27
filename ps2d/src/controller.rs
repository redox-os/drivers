use syscall::io::{Io, Pio, ReadOnly, WriteOnly};

use std::{fmt, thread};

#[derive(Debug)]
pub enum Error {
    CommandRetry,
    NoMoreTries,
    ReadTimeout,
    WriteTimeout,
}

bitflags! {
    pub struct StatusFlags: u8 {
        const OUTPUT_FULL = 1;
        const INPUT_FULL = 1 << 1;
        const SYSTEM = 1 << 2;
        const COMMAND = 1 << 3;
        // Chipset specific
        const KEYBOARD_LOCK = 1 << 4;
        // Chipset specific
        const SECOND_OUTPUT_FULL = 1 << 5;
        const TIME_OUT = 1 << 6;
        const PARITY = 1 << 7;
    }
}

bitflags! {
    pub struct ConfigFlags: u8 {
        const FIRST_INTERRUPT = 1;
        const SECOND_INTERRUPT = 1 << 1;
        const POST_PASSED = 1 << 2;
        // 1 << 3 should be zero
        const CONFIG_RESERVED_3 = 1 << 3;
        const FIRST_DISABLED = 1 << 4;
        const SECOND_DISABLED = 1 << 5;
        const FIRST_TRANSLATE = 1 << 6;
        // 1 << 7 should be zero
        const CONFIG_RESERVED_7 = 1 << 7;
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
#[allow(dead_code)]
enum Command {
    ReadConfig = 0x20,
    WriteConfig = 0x60,
    DisableSecond = 0xA7,
    EnableSecond = 0xA8,
    TestSecond = 0xA9,
    TestController = 0xAA,
    TestFirst = 0xAB,
    Diagnostic = 0xAC,
    DisableFirst = 0xAD,
    EnableFirst = 0xAE,
    WriteSecond = 0xD4
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
#[allow(dead_code)]
enum KeyboardCommand {
    EnableReporting = 0xF4,
    SetDefaultsDisable = 0xF5,
    SetDefaults = 0xF6,
    Reset = 0xFF
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
enum KeyboardCommandData {
    ScancodeSet = 0xF0
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
#[allow(dead_code)]
enum MouseCommand {
    GetDeviceId = 0xF2,
    EnableReporting = 0xF4,
    SetDefaultsDisable = 0xF5,
    SetDefaults = 0xF6,
    Reset = 0xFF
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
enum MouseCommandData {
    SetSampleRate = 0xF3,
}

pub struct Ps2 {
    data: Pio<u8>,
    status: ReadOnly<Pio<u8>>,
    command: WriteOnly<Pio<u8>>
}

impl Ps2 {
    pub fn new() -> Self {
        Ps2 {
            data: Pio::new(0x60),
            status: ReadOnly::new(Pio::new(0x64)),
            command: WriteOnly::new(Pio::new(0x64)),
        }
    }

    fn status(&mut self) -> StatusFlags {
        StatusFlags::from_bits_truncate(self.status.read())
    }

    fn wait_write(&mut self) -> Result<(), Error> {
        let mut timeout = 100_000;
        while self.status().contains(StatusFlags::INPUT_FULL) {
            if timeout <= 0 {
                return Err(Error::WriteTimeout);
            }
            timeout -= 1;
            thread::yield_now();
        }
        Ok(())
    }

    fn wait_read(&mut self) -> Result<(), Error> {
        let mut timeout = 100_000;
        while ! self.status().contains(StatusFlags::OUTPUT_FULL) {
            if timeout <= 0 {
                return Err(Error::ReadTimeout);
            }
            timeout -= 1;
            thread::yield_now();
        }
        Ok(())
    }

    fn flush_read(&mut self, message: &str) {
        while self.status().contains(StatusFlags::OUTPUT_FULL) {
            eprintln!("ps2d: flush {}: {:X}", message, self.data.read());
        }
    }

    fn command(&mut self, command: Command) -> Result<(), Error> {
        self.wait_write()?;
        self.command.write(command as u8);
        Ok(())
    }

    fn read(&mut self) -> Result<u8, Error> {
        self.wait_read()?;
        Ok(self.data.read())
    }

    fn write(&mut self, data: u8) -> Result<(), Error> {
        self.wait_write()?;
        self.data.write(data);
        Ok(())
    }

    fn config(&mut self) -> Result<ConfigFlags, Error> {
        self.command(Command::ReadConfig)?;
        self.read().map(ConfigFlags::from_bits_truncate)
    }

    fn set_config(&mut self, config: ConfigFlags) -> Result<(), Error> {
        self.command(Command::WriteConfig)?;
        self.write(config.bits())
    }

    fn retry<F: Fn(&mut Self) -> Result<u8, Error>>(&mut self, name: fmt::Arguments, retries: usize, f: F) -> Result<u8, Error> {
        let mut res = Err(Error::NoMoreTries);
        for retry in 0..retries {
            res = f(self);
            match res {
                Ok(ok) => {
                    return Ok(ok);
                },
                Err(ref err) => {
                    eprintln!("ps2d: {}: retry {}/{}: {:?}", name, retry + 1, retries, err);
                }
            }
        }
        res
    }

    fn keyboard_command_inner(&mut self, command: u8) -> Result<u8, Error> {
        self.write(command as u8)?;
        match self.read()? {
            0xFE => Err(Error::CommandRetry),
            value => Ok(value),
        }
    }

    fn keyboard_command(&mut self, command: KeyboardCommand) -> Result<u8, Error> {
        self.retry(
            format_args!("{:?}", command),
            4,
            |x| x.keyboard_command_inner(command as u8)
        )
    }

    fn keyboard_command_data(&mut self, command: KeyboardCommandData, data: u8) -> Result<u8, Error> {
        self.retry(
            format_args!("{:?} {:#x}", command, data),
            4,
            |x| {
                let res = x.keyboard_command_inner(command as u8)?;
                if res != 0xFA {
                    //TODO: error?
                    return Ok(res);
                }
                x.write(data);
                x.read()
            }
        )
    }

    fn mouse_command_inner(&mut self, command: u8) -> Result<u8, Error> {
        self.command(Command::WriteSecond)?;
        self.write(command as u8)?;
        match self.read()? {
            0xFE => Err(Error::CommandRetry),
            value => Ok(value),
        }
    }

    fn mouse_command(&mut self, command: MouseCommand) -> Result<u8, Error> {
        self.retry(
            format_args!("{:?}", command),
            4,
            |x| x.mouse_command_inner(command as u8)
        )
    }

    fn mouse_command_data(&mut self, command: MouseCommandData, data: u8) -> Result<u8, Error> {
        self.retry(
            format_args!("{:?} {:#x}", command, data),
            4,
            |x| {
                let res = x.mouse_command_inner(command as u8)?;
                if res != 0xFA {
                    //TODO: error?
                    return Ok(res);
                }
                x.command(Command::WriteSecond)?;
                x.write(data as u8)?;
                x.read()
            }
        )
    }

    pub fn next(&mut self) -> Option<(bool, u8)> {
        let status = self.status();
        if status.contains(StatusFlags::OUTPUT_FULL) {
            let data = self.data.read();
            Some((! status.contains(StatusFlags::SECOND_OUTPUT_FULL), data))
        } else {
            None
        }
    }

    pub fn init_mouse(&mut self) -> Result<bool, Error> {
        let mut b;

        // Clear remaining data
        self.flush_read("init mouse start");

        // Wake up mouse by reading device ID
        b = self.mouse_command(MouseCommand::GetDeviceId)?;
        if b == 0xFA {
            b = self.read()?;
        } else {
            eprintln!("ps2d: failed to get mouse device id: {:02X}", b);
        }

        // Clear remaining data
        self.flush_read("mouse device id");

        // Reset mouse and set up scroll
        b = self.mouse_command(MouseCommand::Reset)?;
        if b == 0xFA {
            b = self.read()?;
            if b != 0xAA {
                eprintln!("ps2d: mouse failed self test 1: {:02X}", b);
            }

            b = self.read()?;
            if b != 0x00 {
                eprintln!("ps2d: mouse failed self test 2: {:02X}", b);
            }
        } else {
            eprintln!("ps2d: mouse failed to reset: {:02X}", b);
        }

        // Clear remaining data
        self.flush_read("mouse reset");

        // Set defaults
        b = self.mouse_command(MouseCommand::SetDefaults)?;
        if b != 0xFA {
            eprintln!("ps2d: mouse failed to set defaults: {:02X}", b);
        }

        // Clear remaining data
        self.flush_read("mouse defaults");

        // Enable extra packet on mouse
        //TODO: show error return values
        if self.mouse_command_data(MouseCommandData::SetSampleRate, 200)? != 0xFA
        || self.mouse_command_data(MouseCommandData::SetSampleRate, 100)? != 0xFA
        || self.mouse_command_data(MouseCommandData::SetSampleRate, 80)? != 0xFA {
            eprintln!("ps2d: mouse failed to enable extra packet");
        }

        b = self.mouse_command(MouseCommand::GetDeviceId)?;
        let mouse_extra = if b == 0xFA {
            self.read()? == 3
        } else {
            eprintln!("ps2d: mouse failed to get device id: {:02X}", b);
            false
        };

        // Set sample rate to maximum
        let sample_rate = 200;
        b = self.mouse_command_data(MouseCommandData::SetSampleRate, sample_rate)?;
        if b != 0xFA {
            eprintln!("ps2d: mouse failed to set sample rate to {}: {:02X}", sample_rate, b);
        }

        // Enable data reporting
        b = self.mouse_command(MouseCommand::EnableReporting)?;
        if b != 0xFA {
            eprintln!("ps2d: mouse failed to enable reporting: {:02X}", b);
        }

        // Clear remaining data
        self.flush_read("init mouse finish");

        Ok(mouse_extra)
    }

    pub fn init(&mut self) -> Result<bool, Error> {
        let mut b;

        // Clear remaining data
        self.flush_read("init start");

        // Disable devices
        self.command(Command::DisableFirst)?;
        self.command(Command::DisableSecond)?;

        // Clear remaining data
        self.flush_read("disable");

        // Disable clocks, disable interrupts, and disable translate
        {
            let mut config = self.config()?;
            config.insert(ConfigFlags::FIRST_DISABLED);
            config.insert(ConfigFlags::SECOND_DISABLED);
            config.remove(ConfigFlags::FIRST_TRANSLATE);
            config.remove(ConfigFlags::FIRST_INTERRUPT);
            config.remove(ConfigFlags::SECOND_INTERRUPT);
            self.set_config(config)?;
        }

        // Perform the self test
        self.command(Command::TestController)?;
        assert_eq!(self.read()?, 0x55);

        // Clear remaining data
        self.flush_read("test controller");

        // Enable devices
        self.command(Command::EnableFirst)?;
        self.command(Command::EnableSecond)?;

        // Clear remaining data
        self.flush_read("init keyboard start");

        // Reset keyboard
        b = self.keyboard_command(KeyboardCommand::Reset)?;
        if b == 0xFA {
            b = self.read()?;
            if b != 0xAA {
                eprintln!("ps2d: keyboard failed self test: {:02X}", b);
            }
        } else {
            eprintln!("ps2d: keyboard failed to reset: {:02X}", b);
        }

        // Clear remaining data
        self.flush_read("keyboard defaults");

        // Set scancode set to 2
        let scancode_set = 2;
        b = self.keyboard_command_data(KeyboardCommandData::ScancodeSet, scancode_set)?;
        if b != 0xFA {
            eprintln!("ps2d: keyboard failed to set scancode set {}: {:02X}", scancode_set, b);
        }

        // Enable data reporting
        b = self.keyboard_command(KeyboardCommand::EnableReporting)?;
        if b != 0xFA {
            eprintln!("ps2d: keyboard failed to enable reporting: {:02X}", b);
        }

        // Clear remaining data
        self.flush_read("init keyboard finish");

        let (mouse_found, mouse_extra) = match self.init_mouse() {
            Ok(ok) => (true, ok),
            Err(err) => {
                eprintln!("p2sd: failed to initialize mouse: {:?}", err);
                (false, false)
            }
        };

        // Enable clocks and interrupts
        {
            let mut config = self.config()?;
            config.remove(ConfigFlags::FIRST_DISABLED);
            config.insert(ConfigFlags::FIRST_TRANSLATE);
            config.insert(ConfigFlags::FIRST_INTERRUPT);
            if mouse_found {
                config.remove(ConfigFlags::SECOND_DISABLED);
                config.insert(ConfigFlags::SECOND_INTERRUPT);
            } else {
                config.insert(ConfigFlags::SECOND_DISABLED);
                config.remove(ConfigFlags::SECOND_INTERRUPT);
            }
            self.set_config(config)?;
        }

        // Clear remaining data
        self.flush_read("init finish");

        Ok(mouse_extra)
    }
}
