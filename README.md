# Drivers

This document covers the driver details/status.

Implemented devices/hardware interfaces:

- ac97d - Realtek audio chipsets.
- acpid - ACPI (incomplete).

```
- Lack of drivers for devices controlled using AML
- The AML parser does not work on real hardware
- It doesn't use any ACPI functionality other than S5 shutdown
- Needs to implement something like "determine the battery status" on real hardware (hard)
```

- ahcid - SATA.
- alxd - Atheros ethernet (incomplete).

```
- Lack of datasheet to finish
```

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

```
- Need to determine a way to allocate memory under 16MiB for use in ISA DMA
```

- usbctl - USB control (incomplete).

```
- Missing class drivers for various classes
```

- usbhidd - USB HID (incomplete).

```
- Has tons of descriptors that are possible, not all are supported
```

- usbscsid - USB SCSI (incomplete).

```
- Missing class drivers for various classes
```

- vboxd - VirtualBox guest.
- vesad - VESA.
- virtio-blkd - VirtIO block device (incomplete).
- virtio-core - VirtIO core.
- virtio-gpud - VirtIO GPU device (incomplete).
- virtio-netd - VirtIO net device (incomplete).
- xhcid - xHCI (incomplete).

## Interfaces

This section cover the interfaces used by Redox drivers.

### System Calls

- `iopl` - syscall that sets the I/O privilege level. x86 has four privilege rings (0/1/2/3), of which the kernel runs in ring 0 and userspace in ring 3. IOPL can only be changed by the kernel, for obvious security reasons, and therefore the Redox kernel needs root to set it. It is unique for each process. Processes with IOPL=3 can access I/O ports, and the kernel can access them as well.

### Schemes

- `memory:physical` - allows mapping physical memory frames to driver-accessible virtual memory pages, with various available memory types:
    - `memory:physical`: default memory type (currently writeback)
    - `memory:physical@wb` writeback cached memory
    - `memory:physical@uc`: uncacheable memory
    - `memory:physical@wc`: write-combining memory
- `irq:` - allows getting events from interrupts. It is used primarily by listening for its file descriptors using the `event:` scheme.

## Contributing

### Driver Design

A device driver on Redox is an user-space daemon that use system calls and schemes to work.

For operating systems with monolithic kernels, drivers use internal kernel APIs instead of common program APIs.

If you want to port a driver from a monolithic OS to Redox you will need to rewrite the driver with reverse enginnering of the code logic, because the logic is adapted to internal kernel APIs (it's a hard task if the device is complex, datasheets are more easy).

### Write a Driver

Datasheets are preferable, when they are freely available. Be aware that datasheets are often provided under a [Non-Disclosure Agreement](https://en.wikipedia.org/wiki/Non-disclosure_agreement) from hardware vendors, which can affect the ability to create an MIT-licensed driver.

If you don't have datasheets, we recommend you to do reverse-engineering of available C code on BSD drivers.

You can use the [example](https://gitlab.redox-os.org/redox-os/exampled) driver or read the code of other drivers with the same type of your device.

Before testing your changes, be aware of [this](https://doc.redox-os.org/book/ch09-02-coding-and-building.html#a-note-about-drivers).

### Driver References

If you want to reverse enginner the existing drivers, you can access the BSD code using these links:

- [FreeBSD drivers](https://github.com/freebsd/freebsd-src/tree/main/sys/dev)
- [NetBSD drivers](https://github.com/NetBSD/src/tree/trunk/sys/dev)
- [OpenBSD drivers](https://github.com/openbsd/src/tree/master/sys/dev)
