#[repr(u8)]
pub enum AtaCommand {
    ReadPio = 0x20,
    ReadPioExt = 0x24,
    ReadDma = 0xC8,
    ReadDmaExt = 0x25,
    WritePio = 0x30,
    WritePioExt = 0x34,
    WriteDma = 0xCA,
    WriteDmaExt = 0x35,
    CacheFlush = 0xE7,
    CacheFlushExt = 0xEA,
    Packet = 0xA0,
    IdentifyPacket = 0xA1,
    Identify = 0xEC,
}
