# ixgbed (a.k.a. ixy.rs on Redox)

ixgbed is the Redox port of [ixy.rs](https://github.com/ixy-languages/ixy.rs), a Rust rewrite of the [ixy](https://github.com/emmericp/ixy) userspace network driver.
It is designed to be readable, idiomatic Rust code.
It supports Intel 82599 10GbE NICs (`ixgbe` family).

## Features

* first 10 Gbit/s network driver on Redox
* transmitting 250 times faster than e1000 / rtl8168 driver
* MSI-X interrupts (not supported by Redox yet)
* less than 1000 lines of code for the driver
* documented code

## Build instructions

See the [Redox README](https://gitlab.redox-os.org/redox-os/redox/blob/master/README.md) for build instructions.

To run ixgbed on Redox (in case the driver is not shipped with Redox anymore)

* clone this project into `cookbook/recipes/drivers/source/`
* create an entry for ixgbed in `cookbook/recipes/drivers/source/Cargo.toml`
* check if your ixgbe device is included in `config.toml`
* touch `filesystem.toml` in Redox's root directory, build Redox and run it

## Usage

To test the driver's transmit and forwarding capabilities, have a look at [rheinfall](https://github.com/ackxolotl/rheinfall), a simple packet generator / forwarder application.

## Docs

ixgbed contains documentation that can be created and viewed by running

```
cargo doc --open
```

