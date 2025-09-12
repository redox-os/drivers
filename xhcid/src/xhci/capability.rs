use common::io::{Io, Mmio};

/// Represents the memory-mapped Capability Registers of the XHCI
///
/// These are read-only registers that specify the capabilities
/// of the host controller implementation.
///
/// They are used by the driver to determine what subsystems to
/// configure during initialization.
///
/// See XHCI Section 5.3. Table 5-9 describes the offsets of the registers
/// in memory.
#[repr(C, packed)]
pub struct CapabilityRegs {
    /// The length of the Capability Registers data structure in XHCI memory.
    ///
    /// While only the registers in this structure are defined by the XHCI standard,
    /// the standard defines an arbitrary amount of space following those registers that
    /// are reserved for the standard. As such, you need to know the offset to the operational
    /// registers, which immediately follow.
    ///
    /// CAPLENGTH in XHC Table 5-9. See XHC 5.3.1
    pub len: Mmio<u8>,
    /// Reserved byte
    ///
    /// Rsvd in XHC Table 5-9
    _rsvd: Mmio<u8>,
    /// The XHCI interface version number in Binary-Encoded Decimal.
    ///
    /// This specifies the version of the XHCI specification that is supported by this controller.
    /// HCIVERSION in XHC Table 5-9
    pub hci_ver: Mmio<u16>,
    /// The HCI Structural Parameters 1 Register.
    ///
    /// -Bits 0 - 7 describe the number of device slots supported by this controller
    /// -Bits 8 - 18 describe the number of interrupters supported by this controller
    /// -Bits 19-23 are reserved
    /// -Bits 24-31 specify the maximum number of ports supported by this controller.
    ///
    /// HCPARAMS1 in XHC Table 5-9. See 5.3.3
    pub hcs_params1: Mmio<u32>,
    /// The HCI Structural Parameters 2 Register.
    ///
    /// - Bits 0-3 describe the Isochronus Scheduling Threshold (IST)
    /// - Bits 4-7 describe the Event Ring Segment Table Max (ERST Max). The maximum number of event
    /// ring segment table entries is 2^(ERST Max)
    /// - Bits 8-20 are reserved
    /// - Bits 25-21 describe the high order five bits of the maximum number of scratchpad buffers
    /// - Bit  26 is the Scratchpad Restore Buffer (SPR). (See XHC 4.23.2)
    /// - Bits 26-31 describe the low order five bits of the maximum number of scratchpad buffers
    ///
    /// HCPARAMS2 in XHC Table 5-9. See 5.3.4
    pub hcs_params2: Mmio<u32>,
    /// The HCI Structural Parameters 3 Register.
    ///
    /// - Bits 0-7 describes the worst-case U1 Device Exit Latency. Values are in microseconds, from 00h to 0Ah. 0B-FFh are reserved
    /// - Bits 8-15 are reserved
    /// - Bits 16-31 describe the worst-case U2 Device Exit Latency. Values are in microseconds, from 0000h to 07FFh. 0800-FFFFh are reserved
    ///
    /// HCPARAMS3 in XHC Table 5-9. See XHC 5.3.5
    pub hcs_params3: Mmio<u32>,
    /// The HCI Capability Parameters 1 Register.
    ///
    /// This register defines optional capabilities supported by the xHCI
    ///
    /// - Bit 0 is the 64-bit Address Capability Flag (AC64). 0 = 32-bit pointers, 1 = 64-bit pointers.
    /// - Bit 1 is the Bandwidth Negotation Capability Flag (BNC)
    /// - Bit 2 is the Context Size Flag (CSZ). 0 = 32-byte, 1 = 64-byte Context Data Structures
    /// - Bit 3 is the Port Power Control Flag (PPC). Indicates whether the implementation supports port power control.
    /// - Bit 4 is the Port Indicators Flag (PIND). Indicates whether the XHC root hub supports port indicator control
    /// - Bit 5 is the Light Host Controller Reset Capability Flag (LHRC). Indicates whether the implementation supports a light reset
    /// - Bit 6 is the Latency Tolerance Messaging Capability Flag (LTC). Indicates whether the implementation supports Latency Tolerance Messaging
    /// - Bit 7 is the no Secondary SID Support Flag (NSS). Indicates whether secondary stream ids is supported. 1 = NO, 0 = YES
    /// - Bit 8 is the Parse All Event Data Flag (PAE). (See XHC Table 5-13)
    /// - Bit 9 is the Stopped - Short Packet Capability Flag (SPC). (See XHC 4.6.9)
    /// - Bit 10 is the Stopped EDTLA Capability Flag (SEC). (See XHC 4.6.9, 4.12, and 6.4.4.1)
    /// - Bit 11 is the Contiguous Frame ID Capability Flag (CFC). (See XHC 4.11.2.5)
    /// - Bits 12-15 are the Maximum Primary Stream Array Size (MaxPSASize). Identifies the maximum size of PSA that the implementation supports.
    /// - Bits 16-31 The xHCI Extended Capabilities Pointer (xECP). Points to an extended capabilities list. (See XHC Table 5-13 to see how to process this value)
    ///
    /// HCCPARAMS1 in XHC Table 5-9. See XHC 5.3.6
    pub hcc_params1: Mmio<u32>,
    /// The Doorbell Offset Register
    ///
    /// This register defines the offset of the Doorbell Array base address from the Base.
    ///
    /// Bits 0-1 are reserved.
    /// Bits 2-31 contain the offset.
    ///
    /// DBOFF in XHC Table 5-9. See XHC 5.3.7
    pub db_offset: Mmio<u32>,
    /// The Runtime Register Space Offset
    ///
    /// The offset of the xHCI Runtime Registers from the Base.
    ///
    /// - Bits 0-4 are reserved.
    /// - Bits 5-31 contain the offset.
    ///
    /// RTSOFF in XHC Table 5-9. See XHC 5.3.8
    pub rts_offset: Mmio<u32>,
    /// The HC Capability Parameters 2 Register
    ///
    /// This register defines optional capabilities supported by the xHCI
    ///
    /// - Bit 0 is the UC3 Entry Capability Flag (U3C). See XHC 4.15.1
    /// - Bit 1 is the Configure Endpoint Command Max Latency Too Large Capability Flag (CMC). See XHC 4.23.5.2 and 5.4.1
    /// - Bit 2 is the Force Save Context Capability (FCS). See XHC 4.23.2 and 5.4.1
    /// - Bit 3 is the Compliance Transition Capability (CTC). See XHC 4.19.2.4.1
    /// - Bit 4 is the Large ESIT Payload Capability (LEC). See XHC 6.2.3.8
    /// - Bit 5 is the Configuration Information Capability (CIC). See XHC 6.2.5.1
    /// - Bit 6 is the Extended TBC Capability (ETC). See XHC 4.11.2.3
    /// - Bit 7 is the Extended TBC TRB Status Capability (ETC_TSC). See XHC 4.11.2.3
    /// - Bit 8 is the Get/Set Extended Property Capability (GSC). See Sections XHC 4.6.17 and 4.6.18
    /// - Bits 10-31 are reserved.
    pub hcc_params2: Mmio<u32>,
    //TODO: VTIOSOFF register for I/O virtualization
}

/// The mask to use to get the AC64 bit from HCCPARAMS1. See [CapabilityRegs]
pub const HCC_PARAMS1_AC64_BIT: u32 = 1 << HCC_PARAMS1_AC64_SHIFT;
/// The shift to use to get the AC64 bit from HCCParams1. See [CapabilityRegs]
pub const HCC_PARAMS1_AC64_SHIFT: u8 = 0;
/// The mask to use to get the CSZ bit from HCCPARAMS1. See [CapabilityRegs]
pub const HCC_PARAMS1_CSZ_BIT: u32 = 1 << HCC_PARAMS1_CSZ_SHIFT;
/// The shift to use to get the CSZ bit from HCCParams1. See [CapabilityRegs]
pub const HCC_PARAMS1_CSZ_SHIFT: u8 = 2;
/// The Mask to use to get the MAXPSASIZE value from HCCParams1. See [CapabilityRegs]
pub const HCC_PARAMS1_MAXPSASIZE_MASK: u32 = 0xF000; // 15:12
/// The shift to use to get the MAXPSASIZE value from HCCParams1. See [CapabilityRegs]
pub const HCC_PARAMS1_MAXPSASIZE_SHIFT: u8 = 12;
/// The mask to use to get the XECP value from HCCParams1. See [CapabilityRegs]
pub const HCC_PARAMS1_XECP_MASK: u32 = 0xFFFF_0000;
/// The shift to use to get the XECP value from HCCParams1. See [CapabilityRegs]
pub const HCC_PARAMS1_XECP_SHIFT: u8 = 16;

/// The mask to use to get the LEC bit from HCCParams2. See [CapabilityRegs]
pub const HCC_PARAMS2_LEC_BIT: u32 = 1 << 4;
/// The mask to use to get the CIC bit from HCCParams2. See [CapabilityRegs]
pub const HCC_PARAMS2_CIC_BIT: u32 = 1 << 5;
/// The mask to use to get MAXPORTS from HCSParams1. See [CapabilityRegs]
pub const HCS_PARAMS1_MAX_PORTS_MASK: u32 = 0xFF00_0000;
/// The shift to use to get MAXPORTS from HCSParams1. See [CapabilityRegs]
pub const HCS_PARAMS1_MAX_PORTS_SHIFT: u8 = 24;
/// The shift to use to get MAXSLOTS from HCSParams1. See [CapabilityRegs]
pub const HCS_PARAMS1_MAX_SLOTS_MASK: u32 = 0x0000_00FF;
/// The shift to use to get MAXSLOTS from HCSParams1. See [CapabilityRegs]
pub const HCS_PARAMS1_MAX_SLOTS_SHIFT: u8 = 0;
/// The mask to use to get MAXSCRATPADBUFS_LO from HCSParams2. See [CapabilityRegs]
pub const HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_LO_MASK: u32 = 0xF800_0000;
/// The shift to use to get MAXSCRATCHPADBUFS_LO from HCSParams2. See [CapabilityRegs]
pub const HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_LO_SHIFT: u8 = 27;
/// The mask to use to get the SPR bit from HCSParams2. See [CapabilityRegs]
pub const HCS_PARAMS2_SPR_BIT: u32 = 1 << HCS_PARAMS2_SPR_SHIFT;
/// The shift to use to get the SPR bit from HCSParams2. See [CapabilityRegs]
pub const HCS_PARAMS2_SPR_SHIFT: u8 = 26;
/// The mask to use to get MAXSCRATCHPADBUFS_HI from HCSParams2. See [CapabilityRegs]
pub const HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_HI_MASK: u32 = 0x03E0_0000;
/// The shift to use to get MAXSCRATCHPADBUFS_HI from HCSParams2. See [CapabilityRegs]

pub const HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_HI_SHIFT: u8 = 21;

impl CapabilityRegs {
    /// Gets the ACS64 bit from HCCParams1.
    pub fn ac64(&self) -> bool {
        self.hcc_params1.readf(HCC_PARAMS1_AC64_BIT)
    }

    /// Gets the context size (CSZ) bit from HCCParams1.
    pub fn csz(&self) -> bool {
        self.hcc_params1.readf(HCC_PARAMS1_CSZ_BIT)
    }

    /// Gets the LEC bit from HCCParams2.
    pub fn lec(&self) -> bool {
        self.hcc_params2.readf(HCC_PARAMS2_LEC_BIT)
    }
    /// Gets the CIC bit from HCCParams2.
    pub fn cic(&self) -> bool {
        self.hcc_params2.readf(HCC_PARAMS2_CIC_BIT)
    }

    /// Gets the Max PSA Size from HCCParams1
    pub fn max_psa_size(&self) -> u8 {
        ((self.hcc_params1.read() & HCC_PARAMS1_MAXPSASIZE_MASK) >> HCC_PARAMS1_MAXPSASIZE_SHIFT)
            as u8
    }

    /// Gets the maximum number of ports from HCCParams1
    pub fn max_ports(&self) -> u8 {
        ((self.hcs_params1.read() & HCS_PARAMS1_MAX_PORTS_MASK) >> HCS_PARAMS1_MAX_PORTS_SHIFT)
            as u8
    }

    /// Gets the maximum number of ports from HCCParams 2
    pub fn max_slots(&self) -> u8 {
        (self.hcs_params1.read() & HCS_PARAMS1_MAX_SLOTS_MASK) as u8
    }

    /// Gets the extended capability pointer from HCCParams1 in DWORDs.
    pub fn ext_caps_ptr_in_dwords(&self) -> u16 {
        ((self.hcc_params1.read() & HCC_PARAMS1_XECP_MASK) >> HCC_PARAMS1_XECP_SHIFT) as u16
    }

    /// Gets the lower five bits from the Max Scratchpad Buffer Lo Register in HCSParams2
    pub fn max_scratchpad_bufs_lo(&self) -> u8 {
        ((self.hcs_params2.read() & HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_LO_MASK)
            >> HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_LO_SHIFT) as u8
    }

    /// Gets the SPR register from HCSParams2
    pub fn spr(&self) -> bool {
        self.hcs_params2.readf(HCS_PARAMS2_SPR_BIT)
    }

    /// Gets the higher five bits from the Max Scratchpad Buffer Hi Register in HCSParams2
    pub fn max_scratchpad_bufs_hi(&self) -> u8 {
        ((self.hcs_params2.read() & HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_HI_MASK)
            >> HCS_PARAMS2_MAX_SCRATCHPAD_BUFS_HI_SHIFT) as u8
    }

    /// Gets the maximum number of scratchpad buffers supported by this implementation.
    pub fn max_scratchpad_bufs(&self) -> u16 {
        u16::from(self.max_scratchpad_bufs_lo()) | (u16::from(self.max_scratchpad_bufs_hi()) << 5)
    }
}
