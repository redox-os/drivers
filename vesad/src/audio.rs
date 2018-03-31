use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::{thread, time};

struct Data {
    i: usize,
    buffers: Vec<Box<[u8; 16384]>>,
}

impl Data {
    fn new() -> Data {
        let mut buffers = Vec::new();
        for _i in 0..64 {
            buffers.push(Box::new([0; 16384]));
        }

        Data {
            i: 0,
            buffers: buffers,
        }
    }

    fn queue(&mut self, buf: &[i16]) {
        let mut i = 0;
        for &sample in buf.iter() {
            self.buffers[(self.i + (i + 16383)/16384) % 64][i % 16384] += sample as u8;
            i += 1;
            self.buffers[(self.i + (i + 16383)/16384) % 64][i % 16384] += (sample >> 8) as u8;
            i += 1;
        }
    }

    fn unqueue(&mut self, buf: &mut [u8; 16384]) {
        for i in 0..buf.len() {
            buf[i] = self.buffers[self.i][i];
            self.buffers[self.i][i] = 0;
        }
        self.i = (self.i + 1) % 64;
    }
}

lazy_static! {
    static ref DATA: Mutex<Data> = Mutex::new(Data::new());
}

pub fn queue(buf: &[i16]) {
    let mut data = DATA.lock().unwrap();
    data.queue(buf);
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

    let mut buf = [0; 16384];
    loop {
        {
            let mut data = DATA.lock().unwrap();
            data.unqueue(&mut buf);
        }
        audio.write(&buf).unwrap();
    }
}
