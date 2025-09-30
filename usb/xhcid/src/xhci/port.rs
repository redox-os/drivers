use common::io::{Io, Mmio};

// RO - read-only
// ROS - read-only sticky
// RW - read/write
// RWS - read/write sticky
// RW1CS - read/write-1-to-clear sticky
// RW1S - read/write-1-to-set
// Sticky register values may preserve values through chip hardware reset

bitflags! {
    pub struct PortFlags: u32 {
        const CCS = 1 << 0; // ROS
        const PED = 1 << 1; // RW1CS
        const RSVD_2 = 1 << 2; // RsvdZ
        const OCA = 1 << 3; // RO
        const PR =  1 << 4; // RW1S
        const PLS_0 = 1 << 5; // RWS
        const PLS_1 = 1 << 6; // RWS
        const PLS_2 = 1 << 7; // RWS
        const PLS_3 = 1 << 8; // RWS
        const PP =  1 << 9; // RWS
        const SPEED_0 =  1 << 10; // ROS
        const SPEED_1 =  1 << 11; // ROS
        const SPEED_2 =  1 << 12; // ROS
        const SPEED_3 =  1 << 13; // ROS
        const PIC_AMB = 1 << 14; // RWS
        const PIC_GRN = 1 << 15; // RWS
        const LWS = 1 << 16; // RW
        const CSC = 1 << 17; // RW1CS
        const PEC = 1 << 18; // RW1CS
        const WRC = 1 << 19; // RW1CS
        const OCC = 1 << 20; // RW1CS
        const PRC = 1 << 21; // RW1CS
        const PLC = 1 << 22; // RW1CS
        const CEC = 1 << 23; // RW1CS
        const CAS = 1 << 24; // RO
        const WCE = 1 << 25; // RWS
        const WDE = 1 << 26; // RWS
        const WOE = 1 << 27; // RWS
        const RSVD_28 = 1 << 28; // RsvdZ
        const RSVD_29 = 1 << 29; // RsvdZ
        const DR =  1 << 30; // RO
        const WPR = 1 << 31; // RW1S
    }
}

#[repr(C, packed)]
pub struct Port {
    // This has write one to clear fields, do not expose it, handle writes carefully!
    portsc: Mmio<u32>,
    pub portpmsc: Mmio<u32>,
    pub portli: Mmio<u32>,
    pub porthlpmc: Mmio<u32>,
}

impl Port {
    pub fn read(&self) -> u32 {
        self.portsc.read()
    }

    pub fn clear_csc(&mut self) {
        self.portsc
            .write((self.flags_preserved() | PortFlags::CSC).bits());
    }

    pub fn clear_prc(&mut self) {
        self.portsc
            .write((self.flags_preserved() | PortFlags::PRC).bits());
    }

    pub fn set_pr(&mut self) {
        self.portsc
            .write((self.flags_preserved() | PortFlags::PR).bits());
    }

    pub fn state(&self) -> u8 {
        ((self.read() & (0b1111 << 5)) >> 5) as u8
    }

    pub fn speed(&self) -> u8 {
        ((self.read() & (0b1111 << 10)) >> 10) as u8
    }

    pub fn flags(&self) -> PortFlags {
        PortFlags::from_bits_truncate(self.read())
    }

    // Read only preserved flags
    pub fn flags_preserved(&self) -> PortFlags {
        // RO(S) and RW(S) bits should be preserved
        // RW1S and RW1CS bits should not
        let preserved = PortFlags::CCS
            | PortFlags::OCA
            | PortFlags::PLS_0
            | PortFlags::PLS_1
            | PortFlags::PLS_2
            | PortFlags::PLS_3
            | PortFlags::PP
            | PortFlags::SPEED_0
            | PortFlags::SPEED_1
            | PortFlags::SPEED_2
            | PortFlags::SPEED_3
            | PortFlags::PIC_AMB
            | PortFlags::PIC_GRN
            | PortFlags::WCE
            | PortFlags::WDE
            | PortFlags::WOE
            | PortFlags::DR;

        self.flags() & preserved
    }
}
