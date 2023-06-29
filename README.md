# Drivers

These are the currently implemented devices/hardware interfaces.

- ac97d - Realtek audio chipsets.
- acpid - ACPI.
- ahcid - SATA.
- alxd - Atheros ethernet (incomplete).
- bgad - Bochs emulator/debugger.
- block-io-wrapper - Library used by other drivers.
- e1000d - Intel Gigabit ethernet.
- ided - IDE.
- ihdad - Intel HD Audio chipsets.
- ixgbed - Intel 10 Gigabit ethernet.
- nvmed - NVMe.
- pcid - PCI.
- pcspkrd - PC speaker
- ps2d - PS/2
- rtl8168d - Realtek ethernet.
- sb16d - Sound Blaster audio (incomplete).
- vboxd - VirtualBox guest.
- vesad - VESA.
- xhcid - xHCI (incomplete).
- usbctl - USB control (incomplete).
- usbhidd - USB HID (incomplete).
- usbscsid - USB SCSI (incomplete).
- virtio-* - VirtIO (incomplete) (`virtio-blk`, `virtio-net`).

## Contributing to Drivers

If you want to write drivers for Redox, datasheets are preferable, when they are freely available. Be aware that datasheets are often provided under a [Non-Disclosure Agreement](https://en.wikipedia.org/wiki/Non-disclosure_agreement) from hardware vendors, which can affect the ability to create an MIT-licensed driver.

If you don't have datasheets, we recommend you to do reverse-engineering of available C code of BSD drivers.

We recommend BSDs drivers because BSD license is compatible with MIT (permissive), that way we can reuse the code in other drivers.
