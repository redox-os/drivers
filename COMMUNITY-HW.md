# Community Hardware

This document covers the devices from the community that needs a driver.

Unfortunately we can't know the most sold device models of the world to measure our device porting priority, thus we will use our community data to measure our device priorities, if you find a "device model users" survey (similar to [Debian Popularity Contest](https://popcon.debian.org/) and [Steam Hardware/Software Survey](https://store.steampowered.com/hwsurvey/Steam-Hardware-Software-Survey-Welcome-to-Steam)), please comment.

If you want to contribute to this table, install [pciutils](https://mj.ucw.cz/sw/pciutils/) on your Linux distribution (it should have a package on your distribution), run `lspci -v` to see your hardware devices, their kernel drivers and give the results of these items on each device:

- The first field (each device has an unique name for this item)
- Kernel driver in use
- Kernel modules

If you are unsure of what to do, you can talk with us on the [chat](https://doc.redox-os.org/book/chat.html).

## Template

You will use this template to insert your devices on the table.

```
|  |  |  | No |
```

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
| Intel Corporation Raptor Lake-P 6p+8e cores Host Bridge/DRAM Controller | No | No | No |
| Intel Corporation Raptor Lake PCI Express 5.0 Graphics Port (PEG010) (prog-if 00 [Normal decode]) | pcieport | No | No |
| Intel Corporation Raptor Lake-P [UHD Graphics] (rev 04) (prog-if 00 [VGA controller]) | i915 | i915 | No |
| Intel Corporation Raptor Lake Dynamic Platform and Thermal Framework Processor Participant | proc_thermal_pci | processor_thermal_device_pci | No |
| Intel Corporation Raptor Lake PCIe 4.0 Graphics Port (prog-if 00 [Normal decode]) | pcieport | No | No |
| Intel Corporation Raptor Lake-P Thunderbolt 4 PCI Express Root Port #0 (prog-if 00 [Normal decode]) | pcieport | No | No |
| Intel Corporation GNA Scoring Accelerator module | No | No | No |
| Intel Corporation Raptor Lake-P Thunderbolt 4 USB Controller (prog-if 30 [XHCI]) | xhci_hcd | xhci_pci | No |
| Intel Corporation Raptor Lake-P Thunderbolt 4 NHI #0 (prog-if 40 [USB4 Host Interface]) | thunderbolt | thunderbolt | No |
| Intel Corporation Raptor Lake-P Thunderbolt 4 NHI #1 (prog-if 40 [USB4 Host Interface]) | thunderbolt | thunderbolt | No |
| Intel Corporation Alder Lake PCH USB 3.2 xHCI Host Controller (rev 01) (prog-if 30 [XHCI]) | xhci_hcd | xhci_pci | No |
| Intel Corporation Alder Lake PCH Shared SRAM (rev 01) | No | No | No |
| Intel Corporation Raptor Lake PCH CNVi WiFi (rev 01) | iwlwifi | iwlwifi | No |
| Intel Corporation Alder Lake PCH Serial IO I2C Controller #0 (rev 01) | intel-lpss | intel_lpss_pci | No |
| Intel Corporation Alder Lake PCH HECI Controller (rev 01) | mei_me | mei_me | No |
| Intel Corporation Device 51b8 (rev 01) (prog-if 00 [Normal decode]) | pcieport | No | No |
| Intel Corporation Alder Lake-P PCH PCIe Root Port #6 (rev 01) (prog-if 00 [Normal decode]) | pcieport | No | No |
| Intel Corporation Raptor Lake LPC/eSPI Controller (rev 01) | No | No | No |
| Intel Corporation Raptor Lake-P/U/H cAVS (rev 01) (prog-if 80) | sof-audio-pci-intel-tgl | snd_hda_intel, snd_sof_pci_intel_tgl | No |
| Intel Corporation Alder Lake PCH-P SMBus Host Controller | i801_smbus | i2c_i801 | No |
| Intel Corporation Alder Lake-P PCH SPI Controller (rev 01) | intel-spi | spi_intel_pci | No |
| NVIDIA Corporation GA107GLM [RTX A1000 6GB Laptop GPU] (rev a1) | nvidia | nouveau, nvidia_drm, nvidia | No |
| SK hynix Platinum P41/PC801 NVMe Solid State Drive (prog-if 02 [NVM Express]) | nvme | nvme | No |
| Realtek Semiconductor Co., Ltd. RTS5261 PCI Express Card Reader (rev 01) | rtsx_pci | rtsx_pci | No |

<!--
# This is the table template to copy and paste for quick writing
|  |  |  | No |
-->
