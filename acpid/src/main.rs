use std::convert::TryFrom;
use std::mem;
use std::sync::Arc;

mod acpi;
mod aml;

// TODO: Perhaps use the acpi and aml crates?

fn monotonic() -> (u64, u64) {
    use syscall::call::clock_gettime;
    use syscall::data::TimeSpec;
    use syscall::flag::CLOCK_MONOTONIC;

    let mut timespec = TimeSpec::default();

    clock_gettime(CLOCK_MONOTONIC, &mut timespec)
        .expect("failed to fetch monotonic time");

    (timespec.tv_sec as u64, timespec.tv_nsec as u64)
}


fn main() {
    let rxsdt_raw_data: Arc<[u8]> = std::fs::read("kernel/acpi:")
        .expect("acpid: failed to read `kernel/acpi:`")
        .into();

    let sdt = self::acpi::Sdt::new(rxsdt_raw_data)
        .expect("acpid: failed to parse [RX]SDT");

    let mut thirty_two_bit;
    let mut sixty_four_bit;

    let physaddrs_iter = match &sdt.signature {
        b"RSDT" => {
            thirty_two_bit = sdt.data().chunks(mem::size_of::<u32>())
                // TODO: With const generics, the compiler has some way of doing this for static sizes.
                .map(|chunk| <[u8; mem::size_of::<u32>()]>::try_from(chunk).unwrap())
                .map(|chunk| u32::from_le_bytes(chunk))
                .map(u64::from);

            &mut thirty_two_bit as &mut dyn Iterator<Item = u64>
        }
        b"XSDT" => {
            sixty_four_bit = sdt.data().chunks(mem::size_of::<u64>())
                .map(|chunk| <[u8; mem::size_of::<u64>()]>::try_from(chunk).unwrap())
                .map(|chunk| u64::from_le_bytes(chunk));

            &mut sixty_four_bit as &mut dyn Iterator<Item = u64>
        },
        _ => panic!("acpid: expected [RX]SDT from kernel to be either of those"),
    };

}
