# Drivers

This document covers the driver details.

## Hardware Interfaces and Devices

- ac97d - Realtek audio chipsets
- acpid - ACPI interface
- ahcid - SATA interface
- alxd - Atheros ethernet
- amlserde - a library to provide serialization/deserialization of the AML symbol table from ACPI
- bgad - Bochs emulator and debugger
- block-io-wrapper - Library used by other drivers
- e1000d - Intel Gigabit ethernet
- ided - IDE interface
- ihdad - Intel HD Audio chipsets
- inputd - Multiplexes input from multiple input drivers and provides that to Orbital
- ixgbed - Intel 10 Gigabit ethernet
- nvmed - NVMe interface
- pcid - PCI interface with extensions for PCI Express
- ps2d - PS/2 interface
- rtl8139d - Realtek ethernet
- rtl8168d - Realtek ethernet
- sb16d - Sound Blaster audio
- usbctl - USB control
- usbhidd - USB HID
- usbscsid - USB SCSI
- vboxd - VirtualBox guest
- vesad - VESA interface
- virtio-blkd - VirtIO block device
- virtio-core - VirtIO core
- virtio-gpud - VirtIO GPU device
- virtio-netd - VirtIO Network device
- xhcid - xHCI USB controller

Some drivers are work-in-progress and incomplete, read [this](https://gitlab.redox-os.org/redox-os/drivers/-/issues/41) tracking issue to verify.

## System Interfaces

This section cover the interfaces used by Redox drivers.

### System Calls

- `iopl` - syscall that sets the I/O privilege level. x86 has four privilege rings (0/1/2/3), of which the kernel runs in ring 0 and userspace in ring 3. IOPL can only be changed by the kernel, for obvious security reasons, and therefore the Redox kernel needs root to set it. It is unique for each process. Processes with IOPL=3 can access I/O ports, and the kernel can access them as well.

### Schemes

- `/scheme/memory/physical` - allows mapping physical memory frames to driver-accessible virtual memory pages, with various available memory types:
    - `/scheme/memory/physical`: default memory type (currently writeback)
    - `/scheme/memory/physical@wb` writeback cached memory
    - `/scheme/memory/physical@uc`: uncacheable memory
    - `/scheme/memory/physical@wc`: write-combining memory
- `/scheme/irq` - allows getting events from interrupts. It is used primarily by listening for its file descriptors using the `/scheme/event` scheme.

## Contribution Details

### Driver Design

A device driver on Redox is an user-space daemon that use system calls and schemes to work.

For operating systems with monolithic kernels, drivers use internal kernel APIs instead of common program APIs.

If you want to port a driver from a monolithic OS to Redox you will need to rewrite the driver with reverse enginnering of the code logic, because the logic is adapted to internal kernel APIs (it's a hard task if the device is complex, datasheets are more easy).

### Write a Driver

Datasheets are preferable (much more easy depending on device complexity), when they are freely available. Be aware that datasheets are often provided under a [Non-Disclosure Agreement](https://en.wikipedia.org/wiki/Non-disclosure_agreement) from hardware vendors, which can affect the ability to create an MIT-licensed driver.

If you don't have datasheets, we recommend you to do reverse-engineering of available C code on BSD drivers.

### Libraries

You should use the [redox-scheme](https://crates.io/crates/redox-scheme) and [redox_event](https://crates.io/crates/redox_event) crates to create your drivers, you can also read the [example driver](https://gitlab.redox-os.org/redox-os/exampled) or read the code of other drivers with the same type of your device.

Before testing your changes, be aware of [this](https://doc.redox-os.org/book/coding-and-building.html#a-note-about-drivers).

### References

If you want to reverse enginner the existing drivers, you can access the BSD code using these links:

- [FreeBSD drivers](https://github.com/freebsd/freebsd-src/tree/main/sys/dev)
- [NetBSD drivers](https://github.com/NetBSD/src/tree/trunk/sys/dev)
- [OpenBSD drivers](https://github.com/openbsd/src/tree/master/sys/dev)

## How To Contribute

To learn how to contribute to this system component you need to read the following document:

- [CONTRIBUTING.md](https://gitlab.redox-os.org/redox-os/redox/-/blob/master/CONTRIBUTING.md)

## Development

To learn how to do development with this system component inside the Redox build system you need to read the [Build System](https://doc.redox-os.org/book/build-system-reference.html) and [Coding and Building](https://doc.redox-os.org/book/coding-and-building.html) pages.

### How To Build

To build this system component you need to download the Redox build system, you can learn how to do it on the [Building Redox](https://doc.redox-os.org/book/podman-build.html) page.

This is necessary because they only work with cross-compilation to a Redox virtual machine, but you can do some testing from Linux.
