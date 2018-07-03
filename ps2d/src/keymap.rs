use std::u8;
use std::default::Default;


pub const MAX_KEYCODE: usize = 0x79;
pub const N_MOD_COMBOS: usize = 4;
const N_KEYCODES1: usize = 58;
const N_KEYCODES2: usize = 10;


/// Contains keymap, and is also a scheme
#[allow(unused)]
pub struct Keymap {
    map1: [char; N_MOD_COMBOS * N_KEYCODES1], // Keycodes 0-0x39
    map2: [char; N_MOD_COMBOS * N_KEYCODES2], // Keycodes 0x70-0x79
}

impl Default for Keymap {
    /// 'English' layout.
    fn default() -> Keymap {
        Keymap {
            map1: [
                '\0', '\0', '\0', '\0',
                '\x1B', '\x1B', '\0', '\0',
                '1', '!', '1', '!',
                '2', '@', '2', '@',
                '3', '#', '3', '#',
                '4', '$', '4', '$',
                '5', '%', '5', '%',
                '6', '^', '6', '^',
                '7', '&', '7', '&',
                '8', '*', '8', '*',
                '9', '(', '9', '(',           // 10
                '0', ')', '0', ')',
                '-', '_', '-', '_',
                '=', '+', '=', '+',
                '\x7F', '\x7F', '\0', '\0',
                '\t', '\t', '\0', '\0',
                'q', 'Q', 'q', 'Q',           // 0x10
                'w', 'W', 'w', 'W',
                'e', 'E', 'e', 'E',
                'r', 'R', 'r', 'R',
                't', 'T', 't', 'T',           // 20
                'y', 'Y', 'y', 'Y',
                'u', 'U', 'u', 'U',
                'i', 'I', 'i', 'I',
                'o', 'O', 'o', 'O',
                'p', 'P', 'p', 'P',
                '[', '{', '[', '{',
                ']', '}', ']', '}',
                '\n', '\n', '\n', '\n',
                '\0', '\0', '\0', '\0',
                'a', 'A', 'a', 'A',           // 30
                's', 'S', 's', 'S',
                'd', 'D', 'd', 'D',           // 0x20
                'f', 'F', 'f', 'F',
                'g', 'G', 'g', 'G',
                'h', 'H', 'h', 'H',
                'j', 'J', 'j', 'J',
                'k', 'K', 'k', 'K',
                'l', 'L', 'l', 'L',
                ';', ':', ';', ':',
                '\'', '"', '\'', '"',          // 40
                '`', '~', '`', '~',
                '\0', '\0', '\0', '\0',
                '\\', '|', '\\', '|',
                'z', 'Z', 'z', 'Z',
                'x', 'X', 'x', 'X',
                'c', 'C', 'c', 'C',
                'v', 'V', 'v', 'V',
                'b', 'B', 'b', 'B',           // 0x30
                'n', 'N', 'n', 'N',
                'm', 'M', 'm', 'M',           // 50
                ',', '<', ',', '<',
                '.', '>', '.', '>',
                '/', '?', '/', '?',
                '\0', '\0', '\0', '\0',
                '\0', '\0', '\0', '\0',
                '\0', '\0', '\0', '\0',
                ' ', ' ', ' ', ' '],        // 57 / 0x39
            map2: [
                '0', '0', '0', '0',         // keycode: 0x70
                '1', '1', '1', '1',
                '2', '2', '2', '2',
                '3', '3', '3', '3',
                '4', '4', '4', '4',
                '5', '5', '5', '5',
                '6', '6', '6', '6',
                '7', '7', '7', '7',
                '8', '8', '8', '8',
                '9', '9', '9', '9'],
        }
    }
}

const MAP_2_KEYCODE: usize = 0x70;


impl Keymap {
    pub fn get_char(&self, keycode: u8, shift: bool, alt_gr: bool) -> char {
        let keycode = keycode as usize;
        let modifier_index = (((alt_gr as u8) << 1) | (shift as u8)) as usize;
        self.get_char_at(keycode*N_MOD_COMBOS + modifier_index)
    }

    pub fn set_char_at(&mut self, pos: usize, new_char: char)  {
        let keycode = pos / N_MOD_COMBOS;
        let map: &mut [char] = if keycode >= MAP_2_KEYCODE { &mut self.map2 } else { &mut self.map1 };
        let pos = if keycode >= MAP_2_KEYCODE { pos - MAP_2_KEYCODE*N_MOD_COMBOS } else { pos};
        if let Some(c) = map.get_mut(pos) {
            *c = new_char;
        }
    }
    pub fn get_char_at(&self, pos: usize) -> char  {
        let keycode = pos / N_MOD_COMBOS;
        let map: &[char] = if keycode >= MAP_2_KEYCODE { &self.map2 } else { &self.map1 };
        let pos = if keycode >= MAP_2_KEYCODE { pos - MAP_2_KEYCODE*N_MOD_COMBOS } else { pos};
        if let Some(c) = map.get(pos) {
            *c
        } else {
            '\0'
        }
    }
}
