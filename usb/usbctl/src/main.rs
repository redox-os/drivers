use clap::{App, Arg};
use xhcid_interface::{PortId, XhciClientHandle};

fn main() {
    let matches = App::new("usbctl")
        .arg(
            Arg::with_name("SCHEME")
                .takes_value(true)
                .required(true)
                .long("scheme")
                .short("s"),
        )
        .subcommand(
            App::new("port")
                .arg(Arg::with_name("PORT").takes_value(true).required(true))
                .subcommand(App::new("status"))
                .subcommand(
                    App::new("endpoint")
                        .arg(
                            Arg::with_name("ENDPOINT_NUM")
                                .takes_value(true)
                                .required(true),
                        )
                        .subcommand(App::new("status")),
                ),
        )
        .get_matches();

    let scheme = matches.value_of("SCHEME").expect("no scheme");

    if let Some(port_scmd_matches) = matches.subcommand_matches("port") {
        let port = port_scmd_matches
            .value_of("PORT")
            .expect("invalid utf-8 for PORT argument")
            .parse::<PortId>()
            .expect("expected PORT ID");

        let handle = XhciClientHandle::new(scheme.to_owned(), port);

        if let Some(_status_scmd_matches) = port_scmd_matches.subcommand_matches("status") {
            let state = handle.port_state().expect("Failed to get port state");
            println!("{}", state.as_str());
        } else if let Some(endp_scmd_matches) = port_scmd_matches.subcommand_matches("endpoint") {
            let endp_num = endp_scmd_matches
                .value_of("ENDPOINT_NUM")
                .expect("no valid ENDPOINT_NUM")
                .parse::<u8>()
                .expect("expected ENDPOINT_NUM to be an 8-bit integer");
            let mut endp_handle = handle
                .open_endpoint(endp_num)
                .expect("Failed to open endpoint");
            let state = endp_handle.status().expect("Failed to get endpoint state");
            println!("{}", state.as_str());
        }
    }
}
