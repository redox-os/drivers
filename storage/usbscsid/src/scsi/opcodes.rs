#[repr(u8)]
pub enum Opcode {
    TestUnitReady = 0x00,
    /// obsolete
    RezeroUnit = 0x01,
    RequestSense = 0x03,
    FormatUnit = 0x04,
    ReassignBlocks = 0x07,
    /// obsolete
    Read6 = 0x08,
    /// obsolete
    Write6 = 0x0A,
    /// obsolete
    Seek = 0x0B,
    Inquiry = 0x12,
    ModeSelect6 = 0x15,
    /// obsolete
    Reserve6 = 0x16,
    /// obsolete
    Release6 = 0x17,
    ModeSense6 = 0x1A,
    StartStopUnit = 0x1B,
    RecvDiagnosticRes = 0x1C,
    SendDiagnostic = 0x1D,
    ReadCapacity10 = 0x25,
    Read10 = 0x28,
    Write10 = 0x2A,
    /// obsolete
    SeekExt = 0x2B,
    WriteAndVerify10 = 0x2E,
    Verify10 = 0x2F,
    SyncCache10 = 0x35,
    ReadDefectData10 = 0x37,
    WriteBuf10 = 0x3B,
    ReadBuf10 = 0x3C,
    /// obsolete
    ReadLong10 = 0x3E,
    WriteLong10 = 0x3F,
    /// obsolete
    ChangeDef = 0x40,
    WriteSame10 = 0x41,
    Unmap = 0x42,
    Sanitize = 0x48,
    LogSelect = 0x4C,
    LogSense = 0x4D,
    ModeSelect10 = 0x55,
    /// obsolete
    Reserve10 = 0x56,
    /// obsolete
    Release10 = 0x57,
    ModeSense10 = 0x5A,
    PersistentResvIn = 0x5E,
    PersistentResvOut = 0x5F,
    ServiceAction7F = 0x7F,
    Read16 = 0x88,
    Write16 = 0x8A,
    WriteAndVerify16 = 0x8E,
    Verify16 = 0x8F,
    SyncCache16 = 0x91,
    WriteSame16 = 0x93,
    WriteStream16 = 0x9A,
    ReadBuf16 = 0x9B,
    WriteAtomic16 = 0x9C,
    ServiceAction9E = 0x9E,
    ServiceAction9F,
    ReportLuns = 0xA0,
    SecurityProtoIn = 0xA2,
    ServiceActionA3 = 0xA3,
    ServiceActionA4 = 0xA4,
    Read12 = 0xA8,
    Write12 = 0xAA,
    WriteAndVerify12 = 0xAE,
    Verify12 = 0xAF,
    SecurityProtoOut = 0xB5,
    ReadDefectData12 = 0xB7,
}

#[repr(u8)]
pub enum ServiceAction7F {
    Read32 = 0x09,
    Verify32 = 0x0A,
    Write32 = 0x0B,
    WriteAndVerify32 = 0x0C,
    WriteSame32 = 0x0D,
    WriteAtomic32 = 0x18,
}

#[repr(u8)]
pub enum ServiceAction9E {
    ReadCapacity16 = 0x10,
    ReadLong16 = 0x11,
    GetLbaStatus = 0x12,
    StreamControl = 0x14,
    BackgroundControl = 0x15,
    GetStreamStatus = 0x16,
}
#[repr(u8)]
pub enum ServiceAction9F {
    WriteLong16 = 0x11,
}
#[repr(u8)]
pub enum ServiceActionA3 {
    ReportIdentInfo = 0x05,
    ReportSuppOpcodes = 0x0C,
    ReportSuppTaskManFuncs = 0x0D,
    ReportTimestamp = 0x0F,
}
#[repr(u8)]
pub enum ServiceActionA4 {
    SetIdentInfo = 0x06,
    SetTimestamp = 0x0F,
}
