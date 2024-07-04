use bitvec::{field::BitField, order::Msb0, view::BitView};
use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    Stream,
};
use minifb::{Key, Scale, Window, WindowOptions};
use rand::{rngs::ThreadRng, thread_rng, Rng};
use std::{env, error::Error, f32::consts::PI, fs};

const WIDTH: usize = 64;
const HEIGHT: usize = 32;

const KEY_MAPPINGS: [Key; 16] = [
    Key::X,
    Key::Key1,
    Key::Key2,
    Key::Key3,
    Key::Q,
    Key::W,
    Key::E,
    Key::A,
    Key::S,
    Key::D,
    Key::Z,
    Key::C,
    Key::Key4,
    Key::R,
    Key::F,
    Key::V,
];

const FONTS: [u8; 80] = [
    0xF0, 0x90, 0x90, 0x90, 0xF0, // 0
    0x20, 0x60, 0x20, 0x20, 0x70, // 1
    0xF0, 0x10, 0xF0, 0x80, 0xF0, // 2
    0xF0, 0x10, 0xF0, 0x10, 0xF0, // 3
    0x90, 0x90, 0xF0, 0x10, 0x10, // 4
    0xF0, 0x80, 0xF0, 0x10, 0xF0, // 5
    0xF0, 0x80, 0xF0, 0x90, 0xF0, // 6
    0xF0, 0x10, 0x20, 0x40, 0x40, // 7
    0xF0, 0x90, 0xF0, 0x90, 0xF0, // 8
    0xF0, 0x90, 0xF0, 0x10, 0xF0, // 9
    0xF0, 0x90, 0xF0, 0x90, 0x90, // A
    0xE0, 0x90, 0xE0, 0x90, 0xE0, // B
    0xF0, 0x80, 0x80, 0x80, 0xF0, // C
    0xE0, 0x90, 0x90, 0x90, 0xE0, // D
    0xF0, 0x80, 0xF0, 0x80, 0xF0, // E
    0xF0, 0x80, 0xF0, 0x80, 0x80, // F
];
struct Emulator {
    mem: [u8; 4096],
    reg: [u8; 16],
    stack: Vec<u16>,
    pc: u16,
    idx: u16,
    delay: u8,
    sound: u8,
    screen: [u64; 32],
    window: Window,
    fb: [u32; WIDTH * HEIGHT],
    stream: Stream,
    rng: ThreadRng,
}

impl Emulator {
    pub fn new(program: &[u8]) -> Result<Self, Box<dyn Error>> {
        let window = Window::new(
            "CHIP-8",
            WIDTH,
            HEIGHT,
            WindowOptions {
                scale: Scale::X16,
                ..Default::default()
            },
        )?;

        let mut mem = [0; 4096];
        mem[0x50..(0x50 + FONTS.len())].copy_from_slice(&FONTS);
        mem[0x200..(0x200 + program.len())].copy_from_slice(program);

        let device = cpal::default_host()
            .default_output_device()
            .ok_or("unable to get output device")?;
        let config = device.default_output_config()?.config();

        let sample_rate = config.sample_rate.0 as f32;
        let stream = device.build_output_stream(
            &config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut sample_clock = 0f32;
                for sample in data.iter_mut() {
                    sample_clock = (sample_clock + 1.0) % sample_rate;
                    *sample = (sample_clock * 329.0 * 2.0 * PI / sample_rate).sin();
                }
            },
            |e| {
                panic!("{e}");
            },
            None,
        )?;
        stream.pause()?;

        Ok(Self {
            mem,
            reg: [0; 16],
            stack: Vec::with_capacity(12),
            pc: 0x200,
            idx: 0,
            delay: 0,
            sound: 0,
            rng: thread_rng(),
            screen: [0; 32],
            window,
            stream,
            fb: [0; WIDTH * HEIGHT],
        })
    }

    pub fn run(&mut self) -> Result<(), Box<dyn Error>> {
        self.window.set_target_fps(60);

        while self.window.is_open() && !self.window.is_key_down(Key::Escape) {
            self.delay = self.delay.saturating_sub(1);
            if self.sound > 0 {
                self.sound -= 1;
                if self.sound == 0 {
                    self.stream.pause()?;
                }
            }

            // run a bunch of cycles
            for _cycles in 0..100 {
                let bits = self.mem[self.pc as usize..(self.pc + 2) as usize].view_bits::<Msb0>();

                let op = bits[0..4].load_be::<u8>();
                let x = bits[4..8].load_be::<usize>();
                let y = bits[8..12].load_be::<usize>();
                let n = bits[12..].load_be::<u8>();
                let value = bits[8..].load_be::<u8>();
                let address = bits[4..].load_be::<u16>();

                self.pc += 2;

                let key_pressed = self.window.get_keys();
                let key_released = self.window.get_keys_released();

                match (op, value, n) {
                    (0, 0xE0, _) => {
                        // clear
                        self.screen.fill(0);
                        self.blit_and_update()?;
                        break;
                    }
                    (0, 0xEE, _) => {
                        // pop
                        self.pc = self.stack.pop().ok_or("tried to pop an empty stack")?;
                    }
                    (1, _, _) => {
                        // jump
                        self.pc = address;
                    }
                    (2, _, _) => {
                        // call subroutine
                        self.stack.push(self.pc);
                        self.pc = address;
                    }
                    (3, _, _) => {
                        // skip instruction if x equals value
                        if self.reg[x] == value {
                            self.pc += 2;
                        }
                    }
                    (4, _, _) => {
                        // skip instruction if x doesn't equals value
                        if self.reg[x] != value {
                            self.pc += 2;
                        }
                    }
                    (5, _, 0) => {
                        // skip instruction if x equals yj
                        if self.reg[x] == self.reg[y] {
                            self.pc += 2;
                        }
                    }
                    (6, _, _) => {
                        // set x to value
                        self.reg[x] = value;
                    }
                    (7, _, _) => {
                        // add value to x
                        self.reg[x] = self.reg[x].wrapping_add(value);
                    }
                    (8, _, 0) => {
                        // x = y
                        self.reg[x] = self.reg[y];
                    }
                    (8, _, 1) => {
                        // x = x OR y; flag reset
                        self.reg[x] |= self.reg[y];
                        self.reg[0xF] = 0;
                    }
                    (8, _, 2) => {
                        // x = x AND y; flag reset
                        self.reg[x] &= self.reg[y];
                        self.reg[0xF] = 0;
                    }
                    (8, _, 3) => {
                        // x = x XOR y; flag reset
                        self.reg[x] ^= self.reg[y];
                        self.reg[0xF] = 0;
                    }
                    (8, _, 4) => {
                        // x = x + y with CF
                        let (sum, overflow) = self.reg[x].overflowing_add(self.reg[y]);
                        self.reg[x] = sum;
                        self.reg[0xF] = overflow.into();
                    }
                    (8, _, 5) => {
                        // x = x - y with borrow
                        let (diff, overflow) = self.reg[x].overflowing_sub(self.reg[y]);
                        self.reg[x] = diff;
                        self.reg[0xF] = (!overflow).into();
                    }
                    (8, _, 6) => {
                        // x = y >> 1 with shifted bit
                        let res = self.reg[y] >> 1;
                        let flag = self.reg[y] & 1;
                        self.reg[x] = res;
                        self.reg[0xF] = flag;
                    }

                    (8, _, 7) => {
                        // x = y - x with borrow
                        let (value, overflow) = self.reg[y].overflowing_sub(self.reg[x]);
                        self.reg[x] = value;
                        self.reg[0xF] = (!overflow).into();
                    }
                    (8, _, 0xE) => {
                        // x = y << 1 with shifted bit
                        let res = self.reg[y] << 1;
                        let flag = (self.reg[y] & (1 << 7)) >> 7;
                        self.reg[x] = res;
                        self.reg[0xF] = flag;
                    }

                    (9, _, 0) => {
                        // skip instruction if x and y are not equal
                        if self.reg[x] != self.reg[y] {
                            self.pc += 2;
                        }
                    }
                    (0xA, _, _) => {
                        // set index
                        self.idx = address;
                    }
                    (0xB, _, _) => {
                        // jump to address + v0
                        let offset = self.reg[0] as u16;
                        self.pc = address + offset
                    }
                    (0xC, _, _) => {
                        // x = rand() AND NN
                        self.reg[x] = self.rng.gen::<u8>() & value;
                    }
                    (0xD, _, _) => {
                        // draw
                        let x_pos = (self.reg[x] % 64) as usize;
                        let y_pos = (self.reg[y] % 32) as usize;

                        self.reg[0xf] = 0;

                        for i in 0..n as usize {
                            if y_pos + i >= 32 {
                                break;
                            };

                            let b = self.mem[self.idx as usize + i].view_bits::<Msb0>();
                            let row = self.screen[y_pos + i].view_bits_mut::<Msb0>();

                            for j in 0..8 {
                                if x_pos + j >= 64 {
                                    break;
                                }

                                if b[j] {
                                    if row[x_pos + j] {
                                        self.reg[0xf] = 1;
                                        row.set(x_pos + j, false); // true xor true = false
                                    } else {
                                        row.set(x_pos + j, true); // true xor false = true
                                    }
                                }
                            }
                        }
                        self.blit_and_update()?;
                        break;
                    }
                    (0xE, 0x9E, _) => {
                        // skip if x is pressed
                        if key_pressed.contains(&KEY_MAPPINGS[self.reg[x] as usize]) {
                            self.pc += 2;
                        }
                    }
                    (0xE, 0xA1, _) => {
                        // skip if x is not pressed
                        if !key_pressed.contains(&KEY_MAPPINGS[self.reg[x] as usize]) {
                            self.pc += 2;
                        }
                    }
                    (0xF, 0x07, _) => {
                        // set x to delay
                        self.reg[x] = self.delay;
                    }
                    (0xF, 0x0A, _) => {
                        // wait until key; store key in x
                        if let Some(i) =
                            (0u8..0xF).find(|i| key_released.contains(&KEY_MAPPINGS[*i as usize]))
                        {
                            self.reg[x] = i;
                        } else {
                            self.pc -= 2;
                        }
                    }
                    (0xF, 0x15, _) => {
                        // set delay to x
                        self.delay = self.reg[x];
                    }
                    (0xF, 0x18, _) => {
                        // set sound to x
                        self.sound = self.reg[x];
                        if self.sound == 0 {
                            self.stream.pause()?;
                        } else {
                            self.stream.play()?;
                        }
                    }
                    (0xF, 0x1E, _) => {
                        // Add x to index
                        self.idx = self.idx.wrapping_add(self.reg[x] as u16);
                    }
                    (0xF, 0x29, _) => {
                        // Store address for font char x in i
                        self.idx = 0x50 + 5 * self.reg[(x & 0xF) as usize] as u16;
                    }
                    (0xF, 0x33, _) => {
                        // BCD of x into I..3
                        let number = self.reg[x];
                        self.mem[self.idx as usize] = number / 100;
                        self.mem[self.idx as usize + 1] = (number % 100) / 10;
                        self.mem[self.idx as usize + 2] = number % 10;
                    }
                    (0xF, 0x55, _) => {
                        // Store registers till x starting from i
                        for i in 0..=x {
                            self.mem[self.idx as usize] = self.reg[i];
                            self.idx += 1;
                        }
                    }
                    (0xF, 0x65, _) => {
                        // Load registers till x starting from i
                        for i in 0..=x {
                            self.reg[i] = self.mem[self.idx as usize];
                            self.idx += 1;
                        }
                    }

                    _ => return Err("invalid instruction".into()),
                };
            }

            self.window.update();
        }

        Ok(())
    }

    // emulate vsync behavior
    // do not call self.window.update() after this
    fn blit_and_update(&mut self) -> Result<(), Box<dyn Error>> {
        for (y, row) in self.screen.iter().enumerate() {
            for (x, col) in row.view_bits::<Msb0>().iter().enumerate() {
                self.fb[y * WIDTH + x] = if *col { 0xFFFFFFFF } else { 0 };
            }
        }
        self.window.update_with_buffer(&self.fb, WIDTH, HEIGHT)?;

        Ok(())
    }
}
fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args().skip(1).next().ok_or("rom path not provided")?;
    let f = fs::read(path)?;
    let mut emu = Emulator::new(&f)?;
    emu.run()?;
    Ok(())
}
