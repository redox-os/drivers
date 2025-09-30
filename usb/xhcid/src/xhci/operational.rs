use common::io::{Io, Mmio};

/// The XHCI Operational Registers
///
/// These registers specify the operational state of the XHCI device, and are used to receive status
/// messages and transmit commands. These registers are offset from the XHCI base address by the
/// "length" field of the [CapabilityRegs]
///
/// See XHCI section 5.4. Table 5-18 describes the offset of these registers in memory.
#[repr(C, packed)]
pub struct OperationalRegs {
    /// The USB Command Register (USBCMD)
    ///
    /// Describes the command to be executed by the XHCI. Writes to this register case a command
    /// to be executed.
    ///
    /// - Bit  0 is the Run/Stop bit (R/S). Writing a value of 1 stops the xHC from executing the schedule, 1 resumes. Latency is ~16ms at worst. (See XHCI Table 5-20)
    /// - Bit  1 is the Host Controller Reset Bit (HCRST). Used by software to reset the host controller (See XHCI Table 5-20)
    /// - Bit  2 is the Interrupter Enable Bit (INTE). Enables interrupting the host system.
    /// - Bit  3 is the Host System Error Enable Bit (HSEE). Enables out-of-band error signalling to the host.
    /// - Bits 4-6 are reserved.
    /// - Bit  7 is the Light Host Controller Reset Bit (LHCRST). Resets the driver without affecting the state of the ports. Affected by [CapabilityRegs]
    /// - Bit  8 is the Controller Save State Bit (CSS). See XHCI Table 5-20
    /// - Bit  9 is the Controller Restore State Bit (CRS). See XHCI Table 5-20
    /// - Bit 10 is the Enable Wrap Event Bit (EWE). See XHCI Table 5-20
    /// - Bit 11 is the Enable U3 MFINDEX Stop Bit (EU3S). See XHCI Table 5-20
    /// - Bit 12 is reserved.
    /// - Bit 13 is the CEM Enable Bit (CME). See XHCI Table 5-20
    /// - Bit 14 is the Extended TBC Enable Bit (ETE). See XHCI Table 5-20
    /// - Bit 15 is the Extended TBC TRB Status Enable Bit (TSC_En). See XHCI Table 5-20
    /// - Bit 16 is the VTIO Enable Bit (VTIOE). Controls the enable state of the VTIO capability.
    /// - Bits 17-31 are reserved.
    ///
    pub usb_cmd: Mmio<u32>,
    /// The USB Status Register (USBSTS)
    ///
    /// This register indicates pending interrupts and various states of the host controller.
    ///
    /// Software sets a bit to '0' in this register by writing a 1 to it.
    ///
    ///
    pub usb_sts: Mmio<u32>,
    /// The PAGESIZE Register (PAGESIZE)
    ///
    ///
    pub page_size: Mmio<u32>,
    /// Reserved bits (RsvdZ)
    _rsvd: [Mmio<u32>; 2],
    /// The Device Notification Control Register (DNCTRL)
    ///
    ///
    pub dn_ctrl: Mmio<u32>,
    /// The Command Ring Control Register Lower 32 bits (CRCR)
    ///
    ///
    pub crcr_low: Mmio<u32>,
    /// The Command Ring Control Register Upper 32 bits (CRCR)
    ///
    ///
    pub crcr_high: Mmio<u32>,
    /// Reserved bits (RsvdZ)
    _rsvd2: [Mmio<u32>; 4],
    /// Device Context Base Address Array Pointer Lower 32 bits (DCBAAP)
    ///
    ///
    pub dcbaap_low: Mmio<u32>,
    /// Device Context Base Address Array Pointer Upper 32 bits (DCBAAP)
    ///
    ///
    pub dcbaap_high: Mmio<u32>,
    /// The Configure Register (CONFIG)
    ///
    ///
    pub config: Mmio<u32>,
    // The standard has another set of reserved bits from 3C-3FFh here
    // The standard has 400-13FFh has a Port Register Set here (likely defined in port.rs).
}

// Run/stop
pub const USB_CMD_RS: u32 = 1 << 0;
/// Host controller reset
pub const USB_CMD_HCRST: u32 = 1 << 1;
// Interrupter enable
pub const USB_CMD_INTE: u32 = 1 << 2;

/// Host controller halted
pub const USB_STS_HCH: u32 = 1 << 0;
/// Host controller not ready
pub const USB_STS_CNR: u32 = 1 << 11;

/// The mask to get the CIE bit from the Config register. See [OperationalRegs]
pub const OP_CONFIG_CIE_BIT: u32 = 1 << 9;

impl OperationalRegs {
    pub fn cie(&self) -> bool {
        self.config.readf(OP_CONFIG_CIE_BIT)
    }
    pub fn set_cie(&mut self, value: bool) {
        self.config.writef(OP_CONFIG_CIE_BIT, value)
    }
}
