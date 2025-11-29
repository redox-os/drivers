use common::{
    io::{Io, Pio, ReadOnly, WriteOnly},
    timeout::Timeout,
};
use log::{debug, error, trace};

use std::fmt;

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
        const FIRST_INTERRUPT = 1 << 0;
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
    WriteSecond = 0xD4,
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
#[allow(dead_code)]
enum KeyboardCommand {
    EnableReporting = 0xF4,
    SetDefaultsDisable = 0xF5,
    SetDefaults = 0xF6,
    Reset = 0xFF,
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
enum KeyboardCommandData {
    ScancodeSet = 0xF0,
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
#[allow(dead_code)]
enum MouseCommand {
    SetScaling1To1 = 0xE6,
    SetScaling2To1 = 0xE7,
    StatusRequest = 0xE9,
    GetDeviceId = 0xF2,
    EnableReporting = 0xF4,
    SetDefaultsDisable = 0xF5,
    SetDefaults = 0xF6,
    Reset = 0xFF,
}

#[derive(Clone, Copy, Debug)]
#[repr(u8)]
enum MouseCommandData {
    SetSampleRate = 0xF3,
}

// Default timeout in microseconds
const DEFAULT_TIMEOUT: u64 = 50_000;
// Reset timeout in microseconds
const RESET_TIMEOUT: u64 = 500_000;

pub struct Ps2 {
    data: Pio<u8>,
    status: ReadOnly<Pio<u8>>,
    command: WriteOnly<Pio<u8>>,
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

    fn wait_read(&mut self, micros: u64) -> Result<(), Error> {
        let timeout = Timeout::from_micros(micros);
        loop {
            if self.status().contains(StatusFlags::OUTPUT_FULL) {
                return Ok(());
            }
            timeout.run().map_err(|()| Error::ReadTimeout)?
        }
    }

    fn wait_write(&mut self, micros: u64) -> Result<(), Error> {
        let timeout = Timeout::from_micros(micros);
        loop {
            if !self.status().contains(StatusFlags::INPUT_FULL) {
                return Ok(());
            }
            timeout.run().map_err(|()| Error::WriteTimeout)?
        }
    }

    fn command(&mut self, command: Command) -> Result<(), Error> {
        self.wait_write(DEFAULT_TIMEOUT)?;
        self.command.write(command as u8);
        Ok(())
    }

    fn read(&mut self) -> Result<u8, Error> {
        self.read_timeout(DEFAULT_TIMEOUT)
    }

    fn read_timeout(&mut self, micros: u64) -> Result<u8, Error> {
        self.wait_read(micros)?;
        let data = self.data.read();
        Ok(data)
    }

    fn write(&mut self, data: u8) -> Result<(), Error> {
        self.wait_write(DEFAULT_TIMEOUT)?;
        self.data.write(data);
        Ok(())
    }

    fn retry<F: Fn(&mut Self) -> Result<u8, Error>>(
        &mut self,
        name: fmt::Arguments,
        retries: usize,
        f: F,
    ) -> Result<u8, Error> {
        trace!("ps2d: {}", name);
        let mut res = Err(Error::NoMoreTries);
        for retry in 0..retries {
            res = f(self);
            match res {
                Ok(ok) => {
                    return Ok(ok);
                }
                Err(ref err) => {
                    debug!("ps2d: {}: retry {}/{}: {:?}", name, retry + 1, retries, err);
                }
            }
        }
        res
    }

    fn config(&mut self) -> Result<ConfigFlags, Error> {
        self.retry(format_args!("read config"), 4, |x| {
            x.command(Command::ReadConfig)?;
            x.read()
        })
        .map(ConfigFlags::from_bits_truncate)
    }

    fn set_config(&mut self, config: ConfigFlags) -> Result<(), Error> {
        self.retry(format_args!("write config"), 4, |x| {
            x.command(Command::WriteConfig)?;
            x.write(config.bits())?;
            Ok(0)
        })?;
        Ok(())
    }

    fn keyboard_command_inner(&mut self, command: u8) -> Result<u8, Error> {
        self.write(command)?;
        match self.read()? {
            0xFE => Err(Error::CommandRetry),
            value => Ok(value),
        }
    }

    fn keyboard_command(&mut self, command: KeyboardCommand) -> Result<u8, Error> {
        self.retry(format_args!("keyboard command {:?}", command), 4, |x| {
            x.keyboard_command_inner(command as u8)
        })
    }

    fn keyboard_command_data(
        &mut self,
        command: KeyboardCommandData,
        data: u8,
    ) -> Result<u8, Error> {
        self.retry(
            format_args!("keyboard command {:?} {:#x}", command, data),
            4,
            |x| {
                let res = x.keyboard_command_inner(command as u8)?;
                if res != 0xFA {
                    //TODO: error?
                    return Ok(res);
                }
                x.write(data);
                x.read()
            },
        )
    }

    fn mouse_command_inner(&mut self, command: u8) -> Result<u8, Error> {
        self.command(Command::WriteSecond)?;
        self.write(command)?;
        match self.read()? {
            0xFE => Err(Error::CommandRetry),
            value => Ok(value),
        }
    }

    fn mouse_command(&mut self, command: MouseCommand) -> Result<u8, Error> {
        self.retry(format_args!("mouse command {:?}", command), 4, |x| {
            x.mouse_command_inner(command as u8)
        })
    }

    fn mouse_command_data(&mut self, command: MouseCommandData, data: u8) -> Result<u8, Error> {
        self.retry(
            format_args!("mouse command {:?} {:#x}", command, data),
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
            },
        )
    }

    pub fn next(&mut self) -> Option<(bool, u8)> {
        let status = self.status();
        if status.contains(StatusFlags::OUTPUT_FULL) {
            let data = self.data.read();
            Some((!status.contains(StatusFlags::SECOND_OUTPUT_FULL), data))
        } else {
            None
        }
    }

    pub fn init_keyboard(&mut self) -> Result<(), Error> {
        let mut b;

        {
            // Enable first device
            self.command(Command::EnableFirst)?;
        }

        {
            // Reset keyboard
            b = self.keyboard_command(KeyboardCommand::Reset)?;
            if b == 0xFA {
                b = self.read().unwrap_or(0);
                if b != 0xAA {
                    error!("ps2d: keyboard failed self test: {:02X}", b);
                }
            } else {
                error!("ps2d: keyboard failed to reset: {:02X}", b);
            }
        }

        self.retry(format_args!("keyboard defaults"), 4, |x| {
            // Set defaults and disable scanning
            let b = x.keyboard_command(KeyboardCommand::SetDefaultsDisable)?;
            if b != 0xFA {
                error!("ps2d: keyboard failed to set defaults: {:02X}", b);
                return Err(Error::CommandRetry);
            }

            Ok(b)
        })?;

        {
            // Set scancode set to 2
            let scancode_set = 2;
            b = self.keyboard_command_data(KeyboardCommandData::ScancodeSet, scancode_set)?;
            if b != 0xFA {
                error!(
                    "ps2d: keyboard failed to set scancode set {}: {:02X}",
                    scancode_set, b
                );
            }
        }

        Ok(())
    }

    pub fn init_mouse(&mut self) -> Result<bool, Error> {
        {
            // Enable second device
            self.command(Command::EnableSecond)?;
        }

        self.retry(format_args!("mouse reset"), 4, |x| {
            // Reset mouse
            let mut b = x.mouse_command(MouseCommand::Reset)?;
            if b == 0xFA {
                b = x.read_timeout(RESET_TIMEOUT)?;
                if b != 0xAA {
                    error!("ps2d: mouse failed self test 1: {:02X}", b);
                    return Err(Error::CommandRetry);
                }

                b = x.read_timeout(RESET_TIMEOUT)?;
                if b != 0x00 {
                    error!("ps2d: mouse failed self test 2: {:02X}", b);
                    return Err(Error::CommandRetry);
                }
            } else {
                error!("ps2d: mouse failed to reset: {:02X}", b);
                return Err(Error::CommandRetry);
            }

            Ok(b)
        })?;

        {
            // Enable extra packet on mouse
            //TODO: show error return values
            if self.mouse_command_data(MouseCommandData::SetSampleRate, 200)? != 0xFA
                || self.mouse_command_data(MouseCommandData::SetSampleRate, 100)? != 0xFA
                || self.mouse_command_data(MouseCommandData::SetSampleRate, 80)? != 0xFA
            {
                error!("ps2d: mouse failed to enable extra packet");
            }
        }

        let b = self.mouse_command(MouseCommand::GetDeviceId)?;
        let mouse_extra = if b == 0xFA {
            self.read()? == 3
        } else {
            error!("ps2d: mouse failed to get device id: {:02X}", b);
            false
        };

        {
            // Set sample rate to maximum
            let sample_rate = 200;
            let b = self.mouse_command_data(MouseCommandData::SetSampleRate, sample_rate)?;
            if b != 0xFA {
                error!(
                    "ps2d: mouse failed to set sample rate to {}: {:02X}",
                    sample_rate, b
                );
            }
        }

        {
            let b = self.mouse_command(MouseCommand::StatusRequest)?;
            if b != 0xFA {
                error!("ps2d: mouse failed to request status: {:02X}", b);
            } else {
                let a = self.read()?;
                let b = self.read()?;
                let c = self.read()?;

                debug!(
                    "ps2d: mouse status {:#x} resolution {} sample rate {}",
                    a, b, c
                );
            }
        }

        Ok(mouse_extra)
    }

    pub fn init(&mut self) -> bool {
        {
            // Disable devices
            self.command(Command::DisableFirst)
                .expect("ps2d: failed to initialize");
            self.command(Command::DisableSecond)
                .expect("ps2d: failed to initialize");
        }

        // Disable clocks, disable interrupts, and disable translate
        {
            // Since the default config may have interrupts enabled, and the kernel may eat up
            // our data in that case, we will write a config without reading the current one
            let config = ConfigFlags::POST_PASSED
                | ConfigFlags::FIRST_DISABLED
                | ConfigFlags::SECOND_DISABLED;
            trace!("ps2d: config set {:?}", config);
            self.set_config(config).expect("ps2d: failed to initialize");
        }

        {
            // Perform the self test
            self.command(Command::TestController)
                .expect("ps2d: failed to initialize");
            assert_eq!(self.read().expect("ps2d: failed to initialize"), 0x55);
        }

        // Initialize keyboard
        self.init_keyboard().expect("ps2d: failed to initialize");

        // Initialize mouse
        let (mouse_found, mouse_extra) = match self.init_mouse() {
            Ok(ok) => (true, ok),
            Err(err) => {
                error!("ps2d: failed to initialize mouse: {:?}", err);
                (false, false)
            }
        };

        {
            // Enable keyboard data reporting
            // Use inner function to prevent retries
            self.keyboard_command_inner(KeyboardCommand::EnableReporting as u8)
                .expect("ps2d: failed to initialize");
            // Response is ignored since scanning is now on
            //TODO: fix by using interrupts?
        }

        if mouse_found {
            // Enable mouse data reporting
            // Use inner function to prevent retries
            self.mouse_command_inner(MouseCommand::EnableReporting as u8)
                .expect("ps2d: failed to initialize");
            // Response is ignored since scanning is now on
            //TODO: fix by using interrupts?
        }

        // Enable clocks and interrupts
        {
            let config = ConfigFlags::POST_PASSED
                | ConfigFlags::FIRST_INTERRUPT
                | ConfigFlags::FIRST_TRANSLATE
                | if mouse_found {
                    ConfigFlags::SECOND_INTERRUPT
                } else {
                    ConfigFlags::SECOND_DISABLED
                };
            trace!("ps2d: config set {:?}", config);
            self.set_config(config).expect("ps2d: failed to initialize");
        }

        mouse_extra
    }
}
