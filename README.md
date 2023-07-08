# Drivers

These are the currently implemented devices/hardware interfaces.

- ac97d - Realtek audio chipsets.
- acpid - ACPI (incomplete).

Lack of drivers for devices controlled using AML.

The AML parser does not work on real hardware.

It doesn't use any ACPI functionality other than S5 shutdown.

Needs to implement something like "determine the battery status" on real hardware (hard).

- ahcid - SATA.
- alxd - Atheros ethernet (incomplete).

Lack of datasheet to finish.

- amlserde - a library to provide serialization/deserialization of the AML symbol table from ACPI (incomplete).
- bgad - Bochs emulator/debugger.
- block-io-wrapper - Library used by other drivers.
- e1000d - Intel Gigabit ethernet.
- ided - IDE.
- ihdad - Intel HD Audio chipsets.
- inputd - Multiplexes input from multiple input drivers and provides that to Orbital.
- ixgbed - Intel 10 Gigabit ethernet.
- nvmed - NVMe.
- pcid - PCI.
- pcspkrd - PC speaker
- ps2d - PS/2
- rtl8139d - Realtek ethernet.
- rtl8168d - Realtek ethernet.
- sb16d - Sound Blaster audio (incomplete).

Still need to determine a way to allocate memory under 16MiB for use in ISA DMA.

- usbctl - USB control (incomplete).

Missing class drivers for various classes.

- usbhidd - USB HID (incomplete).

Has tons of descriptors that are possible, not all are supported.

- usbscsid - USB SCSI (incomplete).

Missing class drivers for various classes.

- vboxd - VirtualBox guest.
- vesad - VESA.
- virtio-blkd - VirtIO block device (incomplete).
- virtio-core - VirtIO core.
- virtio-gpud - VirtIO GPU device (incomplete).
- virtio-netd - VirtIO net device (incomplete).
- xhcid - xHCI (incomplete).

## Contributing to Drivers

If you want to write drivers for Redox, datasheets are preferable, when they are freely available. Be aware that datasheets are often provided under a [Non-Disclosure Agreement](https://en.wikipedia.org/wiki/Non-disclosure_agreement) from hardware vendors, which can affect the ability to create an MIT-licensed driver.

If you don't have datasheets, we recommend you to do reverse-engineering of available C code of BSD drivers.

We recommend BSDs drivers because BSD license is compatible with MIT (permissive), that way we can reuse the code in other drivers.
