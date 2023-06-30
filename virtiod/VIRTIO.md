## Generic
- [x] Reset the device.
- [x] Set the ACKNOWLEDGE status bit: the guest OS has noticed the device.
- [x] Set the DRIVER status bit: the guest OS knows how to drive the device.
- [x] Setup Interrupts

## Driver Specific
- [x] Read device feature bits, and write the subset of feature bits understood by the OS and driver to the device. During this step the driver MAY read (but MUST NOT write) the device-specific configuration fields to check that it can support the device before accepting it.
- [x] Set the FEATURES_OK status bit. The driver MUST NOT accept new feature bits after this step.
- [x] Re-read device status to ensure the FEATURES_OK bit is still set: otherwise, the device does not support our subset of features and the device is unusable.
- [x] Perform device-specific setup, including discovery of virtqueues for the device, optional per-bus setup, reading and possibly writing the device’s virtio configuration space, and population of virtqueues.
- [x] Set the DRIVER_OK status bit. At this point the device is “live”.

## XXX
- [ ] Mark the deamon as ready.

## Drivers
- [ ] `virtio-blk` (in-progress)
- [ ] `virtio-net`
- [ ] `virtio-gpu`
