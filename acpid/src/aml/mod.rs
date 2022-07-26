//! # AML
//!
//! Code to parse and execute ACPI Machine Language tables.

use std::collections::HashMap;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use syscall::io::{Io, Pio};

use crate::acpi::{AcpiContext, AmlContainingTable, Sdt, SdtHeader};

#[macro_use]
mod parsermacros;

mod namespace;
mod termlist;
mod namespacemodifier;
mod pkglength;
mod namestring;
mod namedobj;
mod dataobj;
mod type1opcode;
mod type2opcode;
mod parser;

use self::parser::AmlExecutionContext;
use self::termlist::parse_term_list;
pub use self::namespace::AmlValue;

#[derive(Debug)]
pub enum AmlError {
    AmlParseError(&'static str),
    AmlInvalidOpCode,
    AmlValueError,
    AmlDeferredLoad,
    AmlFatalError(u8, u16, AmlValue),
    AmlHardFatal
}

pub fn parse_aml_table(acpi_ctx: &AcpiContext, sdt: impl AmlContainingTable) -> Result<Vec<String>, AmlError> {
    parse_aml_with_scope(acpi_ctx, sdt, "\\".to_owned())
}

pub fn parse_aml_with_scope(acpi_ctx: &AcpiContext, sdt: impl AmlContainingTable, scope: String) -> Result<Vec<String>, AmlError> {
    let data = sdt.aml();
    let mut ctx = AmlExecutionContext::new(acpi_ctx, scope);

    parse_term_list(data, &mut ctx)?;

    Ok(ctx.namespace_delta)
}

fn init_aml_table(acpi_ctx: &AcpiContext, sdt: impl AmlContainingTable) {
    match parse_aml_table(acpi_ctx, &sdt) {
        Ok(_) => log::debug!("Table {} parsed successfully", sdt.header().signature()),
        Err(AmlError::AmlParseError(e)) => log::error!("Table {} got parse error: {}", sdt.header().signature(), e),
        Err(AmlError::AmlInvalidOpCode) => log::error!("Table {} got invalid opcode", sdt.header().signature()),
        Err(AmlError::AmlValueError) => log::error!("For table {}: type constraints or value bounds not met", sdt.header().signature()),
        Err(AmlError::AmlDeferredLoad) => log::error!("For table {}: deferred load reached top level", sdt.header().signature()),
        Err(AmlError::AmlFatalError(ty, code, val)) => {
            log::error!("Fatal error occurred for table {}: type={}, code={}, val={:?}", sdt.header().signature(), ty, code, val);
            return;
        },
        Err(AmlError::AmlHardFatal) => {
            log::error!("Hard fatal error occurred for table {}", sdt.header().signature());
            return;
        }
    }
}
pub fn init_namespace(context: &AcpiContext) {
    let dsdt = context.dsdt().expect("could not find any DSDT");

    log::debug!("Found DSDT.");
    init_aml_table(context, dsdt);

    let ssdts = context.ssdts();

    for ssdt in ssdts {
        print!("Found SSDT.");
        init_aml_table(context, ssdt);
    }
}

pub fn set_global_s_state(context: &AcpiContext, state: u8) {
    if state != 5 {
        return;
    }
    let fadt = match context.fadt() {
        Some(fadt) => fadt,
        None =>  {
            log::error!("Cannot set global S-state due to missing FADT.");
            return;
        }
    };

    let port = fadt.pm1a_control_block as u16;
    let mut val = 1 << 13;

    let namespace_guard = context.namespace();

    let namespace = match &*namespace_guard {
        Some(namespace) => namespace,
        None => {
            log::error!("Cannot set global S-state due to missing ACPI namespace");
            return;
        }
    };

    let s5 = match namespace.get("\\_S5") {
        Some(s5) => s5,
        None => {
            log::error!("Cannot set global S-state due to missing \\_S5");
            return;
        }
    };
    let p = match s5.get_as_package() {
        Ok(package) => package,
        Err(error) => {
            log::error!("Cannot set global S-state due to \\_S5 not being a package: {:?}", error);
            return;
        }
    };

    let slp_typa = p[0].get_as_integer(context).expect("SLP_TYPa is not an integer");
    let slp_typb = p[1].get_as_integer(context).expect("SLP_TYPb is not an integer");

    log::info!("Shutdown SLP_TYPa {:X}, SLP_TYPb {:X}", slp_typa, slp_typb);
    val |= slp_typa as u16;

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        log::info!("Shutdown with ACPI outw(0x{:X}, 0x{:X})", port, val);
        Pio::<u16>::new(port).write(val);
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        log::error!("Cannot shutdown with ACPI outw(0x{:X}, 0x{:X}) on this architecture", port, val);
    }

    loop {
        core::hint::spin_loop();
    }
}
