use syscall::io::{Io, Pio, ReadOnly, WriteOnly};

bitflags! {
    flags StatusFlags: u8 {
        const OUTPUT_FULL = 1,
        const INPUT_FULL = 1 << 1,
        const SYSTEM = 1 << 2,
        const COMMAND = 1 << 3,
        // Chipset specific
        const KEYBOARD_LOCK = 1 << 4,
        // Chipset specific
        const SECOND_OUTPUT_FULL = 1 << 5,
        const TIME_OUT = 1 << 6,
        const PARITY = 1 << 7
    }
}

bitflags! {
    flags ConfigFlags: u8 {
        const FIRST_INTERRUPT = 1,
        const SECOND_INTERRUPT = 1 << 1,
        const POST_PASSED = 1 << 2,
        // 1 << 3 should be zero
        const CONFIG_RESERVED_3 = 1 << 3,
        const FIRST_DISABLED = 1 << 4,
        const SECOND_DISABLED = 1 << 5,
        const FIRST_TRANSLATE = 1 << 6,
        // 1 << 7 should be zero
        const CONFIG_RESERVED_7 = 1 << 7,
    }
}

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

#[repr(u8)]
#[allow(dead_code)]
enum KeyboardCommand {
    EnableReporting = 0xF4,
    SetDefaultsDisable = 0xF5,
    SetDefaults = 0xF6,
    Reset = 0xFF
}

#[repr(u8)]
enum KeyboardCommandData {
    ScancodeSet = 0xF0
}

#[repr(u8)]
#[allow(dead_code)]
enum MouseCommand {
    GetDeviceId = 0xF2,
    EnableReporting = 0xF4,
    SetDefaultsDisable = 0xF5,
    SetDefaults = 0xF6,
    Reset = 0xFF
}

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

    fn wait_write(&mut self) {
        while self.status().contains(INPUT_FULL) {}
    }

    fn wait_read(&mut self) {
        while ! self.status().contains(OUTPUT_FULL) {}
    }

    fn flush_read(&mut self, message: &str) {
        while self.status().contains(OUTPUT_FULL) {
            print!("ps2d: flush {}: {:X}\n", message, self.data.read());
        }
    }

    fn command(&mut self, command: Command) {
        self.wait_write();
        self.command.write(command as u8);
    }

    fn read(&mut self) -> u8 {
        self.wait_read();
        self.data.read()
    }

    fn write(&mut self, data: u8) {
        self.wait_write();
        self.data.write(data);
    }

    fn config(&mut self) -> ConfigFlags {
        self.command(Command::ReadConfig);
        ConfigFlags::from_bits_truncate(self.read())
    }

    fn set_config(&mut self, config: ConfigFlags) {
        self.command(Command::WriteConfig);
        self.write(config.bits());
    }

    fn keyboard_command_inner(&mut self, command: u8) -> u8 {
        let mut ret = 0xFE;
        for i in 0..4 {
            self.write(command as u8);
            ret = self.read();
            if ret == 0xFE {
                println!("ps2d: retry keyboard command {:X}: {}", command, i);
            } else {
                break;
            }
        }
        ret
    }

    fn keyboard_command(&mut self, command: KeyboardCommand) -> u8 {
        self.keyboard_command_inner(command as u8)
    }

    fn keyboard_command_data(&mut self, command: KeyboardCommandData, data: u8) -> u8 {
        let res = self.keyboard_command_inner(command as u8);
        if res != 0xFA {
            return res;
        }
        self.write(data as u8);
        self.read()
    }

    fn mouse_command_inner(&mut self, command: u8) -> u8 {
        let mut ret = 0xFE;
        for i in 0..4 {
            self.command(Command::WriteSecond);
            self.write(command as u8);
            ret = self.read();
            if ret == 0xFE {
                println!("ps2d: retry mouse command {:X}: {}", command, i);
            } else {
                break;
            }
        }
        ret
    }

    fn mouse_command(&mut self, command: MouseCommand) -> u8 {
        self.mouse_command_inner(command as u8)
    }

    fn mouse_command_data(&mut self, command: MouseCommandData, data: u8) -> u8 {
        let res = self.mouse_command_inner(command as u8);
        if res != 0xFA {
            return res;
        }
        self.command(Command::WriteSecond);
        self.write(data as u8);
        self.read()
    }

    pub fn next(&mut self) -> Option<(bool, u8)> {
        let status = self.status();
        if status.contains(OUTPUT_FULL) {
            let data = self.data.read();
            Some((! status.contains(SECOND_OUTPUT_FULL), data))
        } else {
            None
        }
    }

    pub fn init(&mut self) -> bool {
        // Clear remaining data
        self.flush_read("init start");

        // Disable devices
        self.command(Command::DisableFirst);
        self.command(Command::DisableSecond);

        // Clear remaining data
        self.flush_read("disable");

        // Disable clocks, disable interrupts, and disable translate
        {
            let mut config = self.config();
            config.insert(FIRST_DISABLED);
            config.insert(SECOND_DISABLED);
            config.remove(FIRST_TRANSLATE);
            config.remove(FIRST_INTERRUPT);
            config.remove(SECOND_INTERRUPT);
            self.set_config(config);
        }

        // Perform the self test
        self.command(Command::TestController);
        assert_eq!(self.read(), 0x55);

        // Enable devices
        self.command(Command::EnableFirst);
        self.command(Command::EnableSecond);

        // Clear remaining data
        self.flush_read("enable");

        // Reset keyboard
        if self.keyboard_command(KeyboardCommand::Reset) == 0xFA {
            if self.read() != 0xAA {
                println!("ps2d: keyboard failed self test");
            }
        } else {
            println!("ps2d: keyboard failed to reset");
        }

        // Clear remaining data
        self.flush_read("keyboard defaults");

        // Set scancode set to 2
        let scancode_set = 2;
        if self.keyboard_command_data(KeyboardCommandData::ScancodeSet, scancode_set) != 0xFA {
            println!("ps2d: keyboard failed to set scancode set {}", scancode_set);
        }

        // Enable data reporting
        if self.keyboard_command(KeyboardCommand::EnableReporting) != 0xFA {
            println!("ps2d: keyboard failed to enable reporting");
        }

        // Reset mouse and set up scroll
        if self.mouse_command(MouseCommand::Reset) == 0xFA {
            let a = self.read();
            let b = self.read();
            if a != 0xAA || b != 0x00 {
                println!("ps2d: mouse failed self test");
            }
        } else {
            println!("ps2d: mouse failed to reset");
        }

        // Clear remaining data
        self.flush_read("mouse defaults");

        // Enable extra packet on mouse
        if self.mouse_command_data(MouseCommandData::SetSampleRate, 200) != 0xFA
        || self.mouse_command_data(MouseCommandData::SetSampleRate, 100) != 0xFA
        || self.mouse_command_data(MouseCommandData::SetSampleRate, 80) != 0xFA {
            println!("ps2d: mouse failed to enable extra packet");
        }

        let mouse_extra = if self.mouse_command(MouseCommand::GetDeviceId) == 0xFA {
            self.read() == 3
        } else {
            println!("ps2d: mouse failed to get device id");
            false
        };

        // Set sample rate to maximum
        let sample_rate = 200;
        if self.mouse_command_data(MouseCommandData::SetSampleRate, sample_rate) != 0xFA {
            println!("ps2d: mouse failed to set sample rate to {}", sample_rate);
        }

        // Enable data reporting
        if self.mouse_command(MouseCommand::EnableReporting) != 0xFA {
            println!("ps2d: mouse failed to enable reporting");
        }

        // Enable clocks and interrupts
        {
            let mut config = self.config();
            config.remove(FIRST_DISABLED);
            config.remove(SECOND_DISABLED);
            config.insert(FIRST_TRANSLATE);
            config.insert(FIRST_INTERRUPT);
            config.insert(SECOND_INTERRUPT);
            self.set_config(config);
        }

        // Clear remaining data
        self.flush_read("init finish");

        mouse_extra
    }
}
