This document covers the devices from the community that needs a driver.

Unfortunately we can't know the most sold device models of the world to measure our device porting priority, thus we will use our community data to measure our device priorities, if you find a "device model users" survey (similar to [Debian Popularity Contest](https://popcon.debian.org/) and [Steam Hardware/Software Survey](https://store.steampowered.com/hwsurvey/Steam-Hardware-Software-Survey-Welcome-to-Steam)), please comment.

If you want to contribute to this table, install [pciutils](https://mj.ucw.cz/sw/pciutils/) on your Linux distribution (it should have a package on your distribution), run `lspci -v` to see your hardware devices, their kernel drivers and give the results of these items on each device:

- The first field (each device has an unique name for this item)
- Kernel driver in use
- Kernel modules

If you are unsure of what to do, you can talk with us on the [chat](https://doc.redox-os.org/book/chat.html).

| **Device model** | **Kernel driver** | **Kernel module** | **There's a Redox driver?** |
|------------------|-------------------|-------------------|-----------------------------|
| Realtek RTL8821CE 802.11ac (Wi-Fi) | rtw_8821ce | rtw88_8821ce | No |
| Intel Ice Lake-LP SPI Controller | intel-spi | spi_intel_pci | No |
| Intel Ice Lake-LP SMBus Controller | i801_smbus | i2c_i801 | No |
| Intel Ice Lake-LP Smart Sound Technology Audio Controller | snd_hda_intel | snd_hda_intel, snd_sof_pci_intel_icl | No |
| Intel Ice Lake-LP Serial IO SPI Controller | intel-lpss | No | No |
| Intel Ice Lake-LP Serial IO UART Controller | intel-lpss | No | No |
| Intel Ice Lake-LP Serial IO I2C Controller | intel-lpss | No | No |
| Ice Lake-LP USB 3.1 xHCI Host Controller | xhci_hcd | No | No |
| Intel Processor Power and Thermal Controller | proc_thermal | processor_thermal_device_pci_legacy | No |
| Intel Device 8a02 | icl_uncore | No | No |
| Iris Plus Graphics G1 (Ice Lake) | i915 | i915 | No |
