use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::{thread, time};

lazy_static! {
    static ref DATA: Mutex<(usize, [[u8; 16384]; 64])> = Mutex::new((0, [[0; 16384]; 64]));
}

pub fn queue(buf: &[u8]) {
    let mut data = DATA.lock().unwrap();
    for i in 0..buf.len() {
        let data_0 = data.0;
        data.1[(data_0 + (i + 16383)/16384) % 64][i % 16384] += buf[i];
    }
}

pub fn thread() {
    let mut audio = loop {
        match OpenOptions::new().write(true).open("audio:") {
            Ok(ok) => {
                eprintln!("opened audio:");
                break ok;
            },
            Err(err) => {
                eprintln!("failed to open audio:: {}", err);
                thread::sleep(time::Duration::new(1, 0));
            }
        }
    };

    let mut buf = vec![0; 16384];
    loop {
        {
            let mut data = DATA.lock().unwrap();
            for i in 0..buf.len() {
                let data_0 = data.0;
                buf[i] = data.1[data_0][i];
                data.1[data_0][i] = 0;
            }
            data.0 = (data.0 + 1) % 64;
        }
        audio.write(&buf).unwrap();
    }
}
