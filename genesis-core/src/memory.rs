mod eeprom;
mod external;

use crate::api::GenesisRegion;
use crate::input::InputState;
use crate::memory::external::ExternalMemory;
use crate::vdp::Vdp;
use crate::ym2612::Ym2612;
use crate::GenesisTimingMode;
use bincode::{Decode, Encode};
use jgenesis_proc_macros::{FakeDecode, FakeEncode};
use jgenesis_traits::num::GetBit;
use regex::Regex;
use smsgg_core::psg::Psg;
use std::mem;
use std::ops::Index;
use std::sync::OnceLock;
use z80_emu::traits::InterruptLine;

#[derive(Debug, Clone, Default, FakeEncode, FakeDecode)]
struct Rom(Vec<u8>);

impl Rom {
    fn get(&self, i: usize) -> Option<u8> {
        self.0.get(i).copied()
    }
}

impl Index<usize> for Rom {
    type Output = u8;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl Index<u32> for Rom {
    type Output = u8;

    fn index(&self, index: u32) -> &Self::Output {
        &self.0[index as usize]
    }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Cartridge {
    rom: Rom,
    external_memory: ExternalMemory,
    rom_address_mask: u32,
    region: GenesisRegion,
}

impl Cartridge {
    pub fn from_rom(
        rom_bytes: Vec<u8>,
        initial_ram_bytes: Option<Vec<u8>>,
        forced_region: Option<GenesisRegion>,
    ) -> Self {
        let region = forced_region.unwrap_or_else(|| match GenesisRegion::from_rom(&rom_bytes) {
            Some(region) => region,
            None => {
                log::warn!("Unable to determine cartridge region from ROM header; using Americas");
                GenesisRegion::Americas
            }
        });
        log::info!("Genesis hardware region: {region:?}");

        let external_memory = ExternalMemory::from_rom(&rom_bytes, initial_ram_bytes);

        // TODO parse more stuff out of header
        let rom_address_mask = (rom_bytes.len() - 1) as u32;
        Self { rom: Rom(rom_bytes), external_memory, rom_address_mask, region }
    }

    fn read_byte(&self, address: u32) -> u8 {
        if let Some(byte) = self.external_memory.read_byte(address) {
            return byte;
        }

        self.rom.get(address as usize).unwrap_or(0xFF)
    }

    fn read_word(&self, address: u32) -> u16 {
        if let Some(word) = self.external_memory.read_word(address) {
            return word;
        }

        u16::from_be_bytes([self.read_byte(address), self.read_byte(address.wrapping_add(1))])
    }

    fn write_byte(&mut self, address: u32, value: u8) {
        self.external_memory.write_byte(address, value);
    }

    fn write_word(&mut self, address: u32, value: u16) {
        self.external_memory.write_word(address, value);
    }
}

const MAIN_RAM_LEN: usize = 64 * 1024;
const AUDIO_RAM_LEN: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
struct Z80BankRegister {
    bank_number: u32,
    current_bit: u8,
}

impl Z80BankRegister {
    const BITS: u8 = 9;

    fn map_to_68k_address(self, z80_address: u16) -> u32 {
        (self.bank_number << 15) | u32::from(z80_address & 0x7FFF)
    }

    fn write_bit(&mut self, bit: bool) {
        self.bank_number = (self.bank_number >> 1) | (u32::from(bit) << (Self::BITS - 1));
    }
}

#[derive(Debug, Clone, Encode, Decode)]
struct Signals {
    z80_busreq: bool,
    z80_reset: bool,
}

impl Default for Signals {
    fn default() -> Self {
        Self { z80_busreq: false, z80_reset: true }
    }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Memory {
    cartridge: Cartridge,
    main_ram: Vec<u8>,
    audio_ram: Vec<u8>,
    z80_bank_register: Z80BankRegister,
    signals: Signals,
}

impl Memory {
    pub fn new(cartridge: Cartridge) -> Self {
        Self {
            cartridge,
            main_ram: vec![0; MAIN_RAM_LEN],
            audio_ram: vec![0; AUDIO_RAM_LEN],
            z80_bank_register: Z80BankRegister::default(),
            signals: Signals::default(),
        }
    }

    pub fn read_word_for_dma(&self, address: u32) -> u16 {
        match address {
            0x000000..=0x3FFFFF => self.cartridge.read_word(address),
            0xE00000..=0xFFFFFF => {
                let addr = (address & 0xFFFF) as usize;
                u16::from_be_bytes([
                    self.main_ram[addr],
                    self.main_ram[addr.wrapping_add(1) & 0xFFFF],
                ])
            }
            _ => 0xFF,
        }
    }

    pub fn take_rom(&mut self) -> Vec<u8> {
        mem::take(&mut self.cartridge.rom).0
    }

    pub fn take_rom_from(&mut self, other: &mut Self) {
        self.cartridge.rom = mem::take(&mut other.cartridge.rom);
    }

    pub fn take_cartridge_ram_if_persistent(&mut self) -> Option<Vec<u8>> {
        self.cartridge.external_memory.take_if_persistent()
    }

    pub fn cartridge_title(&self) -> String {
        static RE: OnceLock<Regex> = OnceLock::new();

        let addr = match self.cartridge.region {
            GenesisRegion::Americas | GenesisRegion::Europe => 0x0150,
            GenesisRegion::Japan => 0x0120,
        };
        let bytes = &self.cartridge.rom.0[addr..addr + 48];
        let title = bytes.iter().copied().map(|b| b as char).collect::<String>();

        let re = RE.get_or_init(|| Regex::new(r" +").unwrap());
        re.replace_all(title.trim(), " ").into()
    }

    pub fn hardware_region(&self) -> GenesisRegion {
        self.cartridge.region
    }

    pub fn cartridge_ram(&self) -> &[u8] {
        self.cartridge.external_memory.get_memory()
    }

    pub fn cartridge_ram_persistent(&self) -> bool {
        self.cartridge.external_memory.is_persistent()
    }

    pub fn get_and_clear_cartridge_ram_dirty(&mut self) -> bool {
        self.cartridge.external_memory.get_and_clear_dirty_bit()
    }

    pub fn reset_z80_signals(&mut self) {
        self.signals = Signals::default();
    }
}

pub struct MainBus<'a> {
    memory: &'a mut Memory,
    vdp: &'a mut Vdp,
    psg: &'a mut Psg,
    ym2612: &'a mut Ym2612,
    input: &'a mut InputState,
    timing_mode: GenesisTimingMode,
    z80_stalled: bool,
}

impl<'a> MainBus<'a> {
    pub fn new(
        memory: &'a mut Memory,
        vdp: &'a mut Vdp,
        psg: &'a mut Psg,
        ym2612: &'a mut Ym2612,
        input: &'a mut InputState,
        timing_mode: GenesisTimingMode,
        z80_stalled: bool,
    ) -> Self {
        Self { memory, vdp, psg, ym2612, input, timing_mode, z80_stalled }
    }

    // TODO remove
    #[allow(clippy::match_same_arms)]
    fn read_io_register(&self, address: u32) -> u8 {
        match address {
            // Version register
            0xA10000 | 0xA10001 => {
                0x20 | (u8::from(self.memory.cartridge.region.version_bit()) << 7)
                    | (u8::from(self.timing_mode == GenesisTimingMode::Pal) << 6)
            }
            0xA10002 | 0xA10003 => self.input.read_p1_data(),
            0xA10004 | 0xA10005 => self.input.read_p2_data(),
            0xA10008 | 0xA10009 => self.input.read_p1_ctrl(),
            0xA1000A | 0xA1000B => self.input.read_p2_ctrl(),
            // TxData registers return 0xFF by default
            0xA1000E | 0xA1000F | 0xA10014 | 0xA10015 | 0xA1001A | 0xA1001B => 0xFF,
            // Other I/O registers return 0x00 by default
            _ => 0x00,
        }
    }

    fn write_io_register(&mut self, address: u32, value: u8) {
        match address {
            0xA10002 | 0xA10003 => {
                self.input.write_p1_data(value);
            }
            0xA10004 | 0xA10005 => {
                self.input.write_p2_data(value);
            }
            0xA10008 | 0xA10009 => {
                self.input.write_p1_ctrl(value);
            }
            0xA1000A | 0xA1000B => {
                self.input.write_p2_ctrl(value);
            }
            _ => {}
        }
    }

    fn read_vdp_byte(&mut self, address: u32) -> u8 {
        match address & 0x1F {
            0x00 | 0x02 => (self.vdp.read_data() >> 8) as u8,
            0x01 | 0x03 => self.vdp.read_data() as u8,
            0x04 | 0x06 => (self.vdp.read_status() >> 8) as u8,
            0x05 | 0x07 => self.vdp.read_status() as u8,
            0x08 | 0x0A => (self.vdp.hv_counter() >> 8) as u8,
            0x09 | 0x0B => self.vdp.hv_counter() as u8,
            0x10..=0x1F => {
                // PSG / unused space; PSG is not readable
                0xFF
            }
            _ => unreachable!("address & 0x1F is always <= 0x1F"),
        }
    }

    fn write_vdp_byte(&mut self, address: u32, value: u8) {
        // Byte-size VDP writes duplicate the byte into a word
        let vdp_word = u16::from_le_bytes([value, value]);
        match address & 0x1F {
            0x00..=0x03 => {
                self.vdp.write_data(vdp_word);
            }
            0x04..=0x07 => {
                self.vdp.write_control(vdp_word);
            }
            0x11 | 0x13 | 0x15 | 0x17 => {
                self.psg.write(value);
            }
            0x10 | 0x12 | 0x14 | 0x16 | 0x18..=0x1F => {}
            _ => unreachable!("address & 0x1F is always <= 0x1F"),
        }
    }
}

// The Genesis has a 24-bit bus, not 32-bit
const ADDRESS_MASK: u32 = 0xFFFFFF;

impl<'a> m68000_emu::BusInterface for MainBus<'a> {
    #[inline]
    fn read_byte(&mut self, address: u32) -> u8 {
        let address = address & ADDRESS_MASK;
        log::trace!("Main bus byte read, address={address:06X}");
        match address {
            0x000000..=0x3FFFFF => self.memory.cartridge.read_byte(address),
            0xA00000..=0xA0FFFF => {
                // Z80 memory map
                // For 68k access, $8000-$FFFF mirrors $0000-$7FFF
                <Self as z80_emu::BusInterface>::read_memory(self, (address & 0x7FFF) as u16)
            }
            0xA10000..=0xA1001F => self.read_io_register(address),
            0xA11100..=0xA11101 => (!self.z80_stalled).into(),
            0xA13000..=0xA130FF => {
                todo!("timer register")
            }
            0xC00000..=0xC0001F => self.read_vdp_byte(address),
            0xE00000..=0xFFFFFF => self.memory.main_ram[(address & 0xFFFF) as usize],
            _ => 0xFF,
        }
    }

    #[inline]
    fn read_word(&mut self, address: u32) -> u16 {
        let address = address & ADDRESS_MASK;
        log::trace!("Main bus word read, address={address:06X}");
        match address {
            0x000000..=0x3FFFFF => self.memory.cartridge.read_word(address),
            0xA00000..=0xA0FFFF => {
                // All Z80 access is byte-size; word reads mirror the byte in both MSB and LSB
                let byte = self.read_byte(address);
                u16::from_le_bytes([byte, byte])
            }
            0xA10000..=0xA1001F => self.read_io_register(address).into(),
            0xA11100..=0xA11101 => {
                // Word reads of Z80 BUSREQ signal mirror the byte in both MSB and LSB
                let byte: u8 = (!self.z80_stalled).into();
                u16::from_le_bytes([byte, byte])
            }
            0xA13000..=0xA130FF => {
                todo!("timer register")
            }
            0xC00000..=0xC00003 => self.vdp.read_data(),
            0xC00004..=0xC00007 => self.vdp.read_status(),
            0xC00008..=0xC0000F => self.vdp.hv_counter(),
            0xE00000..=0xFFFFFF => {
                let ram_addr = (address & 0xFFFF) as usize;
                u16::from_be_bytes([
                    self.memory.main_ram[ram_addr],
                    self.memory.main_ram[(ram_addr + 1) & 0xFFFF],
                ])
            }
            _ => 0xFFFF,
        }
    }

    #[inline]
    // TODO remove
    #[allow(clippy::match_same_arms)]
    fn write_byte(&mut self, address: u32, value: u8) {
        let address = address & ADDRESS_MASK;
        log::trace!("Main bus byte write: address={address:06X}, value={value:02X}");
        match address {
            0x000000..=0x3FFFFF => {
                self.memory.cartridge.write_byte(address, value);
            }
            0xA00000..=0xA0FFFF => {
                // Z80 memory map
                // For 68k access, $8000-$FFFF mirrors $0000-$7FFF
                <Self as z80_emu::BusInterface>::write_memory(
                    self,
                    (address & 0x7FFF) as u16,
                    value,
                );
            }
            0xA10000..=0xA1001F => {
                self.write_io_register(address, value);
            }
            0xA11100..=0xA11101 => {
                self.memory.signals.z80_busreq = value.bit(0);
                log::trace!("Set Z80 BUSREQ to {}", self.memory.signals.z80_busreq);
            }
            0xA11200..=0xA11201 => {
                self.memory.signals.z80_reset = !value.bit(0);
                log::trace!("Set Z80 RESET to {}", self.memory.signals.z80_reset);
            }
            0xA13000..=0xA130FF => {
                todo!("timer register")
            }
            0xC00000..=0xC0001F => {
                self.write_vdp_byte(address, value);
            }
            0xE00000..=0xFFFFFF => {
                self.memory.main_ram[(address & 0xFFFF) as usize] = value;
            }
            _ => {}
        }
    }

    #[inline]
    // TODO remove
    #[allow(clippy::match_same_arms)]
    fn write_word(&mut self, address: u32, value: u16) {
        let address = address & ADDRESS_MASK;
        log::trace!("Main bus word write: address={address:06X}, value={value:02X}");
        match address {
            0x000000..=0x3FFFFF => {
                self.memory.cartridge.write_word(address, value);
            }
            0xA00000..=0xA0FFFF => {
                // Z80 memory map; word-size writes write the MSB as a byte-size write
                self.write_byte(address, (value >> 8) as u8);
            }
            0xA10000..=0xA1001F => {
                self.write_io_register(address, value as u8);
            }
            0xA11100..=0xA11101 => {
                self.memory.signals.z80_busreq = value.bit(8);
                log::trace!("Set Z80 BUSREQ to {}", self.memory.signals.z80_busreq);
            }
            0xA11200..=0xA11201 => {
                self.memory.signals.z80_reset = !value.bit(8);
                log::trace!("Set Z80 RESET to {}", self.memory.signals.z80_reset);
            }
            0xA13000..=0xA130FF => {
                todo!("timer register")
            }
            0xC00000..=0xC00003 => {
                self.vdp.write_data(value);
            }
            0xC00004..=0xC00007 => {
                self.vdp.write_control(value);
            }
            0xE00000..=0xFFFFFF => {
                let ram_addr = (address & 0xFFFF) as usize;
                self.memory.main_ram[ram_addr] = (value >> 8) as u8;
                self.memory.main_ram[(ram_addr + 1) & 0xFFFF] = value as u8;
            }
            _ => {}
        }
    }

    #[inline]
    fn interrupt_level(&self) -> u8 {
        self.vdp.m68k_interrupt_level()
    }

    #[inline]
    fn acknowledge_interrupt(&mut self) {
        self.vdp.acknowledge_m68k_interrupt();
    }
}

impl<'a> z80_emu::BusInterface for MainBus<'a> {
    #[inline]
    // TODO remove
    #[allow(clippy::match_same_arms)]
    fn read_memory(&mut self, address: u16) -> u8 {
        log::trace!("Z80 bus read from {address:04X}");

        match address {
            0x0000..=0x3FFF => {
                // Z80 RAM (mirrored at $2000-$3FFF)
                let address = address & 0x1FFF;
                self.memory.audio_ram[address as usize]
            }
            0x4000..=0x5FFF => {
                // YM2612 registers/ports (mirrored every 4 addresses)
                // All YM2612 reads function identically
                self.ym2612.read_register()
            }
            0x6000..=0x60FF => {
                // Bank number register
                // TODO what should this do on reads?
                0xFF
            }
            0x6100..=0x7EFF => {
                // Unused address space
                0xFF
            }
            0x7F00..=0x7F1F => {
                // VDP ports
                self.read_vdp_byte(address.into())
            }
            0x7F20..=0x7FFF => {
                // Invalid addresses
                0xFF
            }
            0x8000..=0xFFFF => {
                let m68k_addr = self.memory.z80_bank_register.map_to_68k_address(address);
                if !(0xA00000..=0xA0FFFF).contains(&m68k_addr) {
                    <Self as m68000_emu::BusInterface>::read_byte(self, m68k_addr)
                } else {
                    // TODO this should lock up the system
                    panic!(
                        "Z80 attempted to read its own memory from the 68k bus; z80_addr={address:04X}, m68k_addr={m68k_addr:08X}"
                    );
                }
            }
        }
    }

    #[inline]
    fn write_memory(&mut self, address: u16, value: u8) {
        log::trace!("Z80 bus write at {address:04X}");

        match address {
            0x0000..=0x3FFF => {
                // Z80 RAM (mirrored at $2000-$3FFF)
                let address = address & 0x1FFF;
                self.memory.audio_ram[address as usize] = value;
            }
            0x4000..=0x5FFF => {
                // YM2612 registers/ports (mirrored every 4 addresses)
                match address & 0x03 {
                    0x00 => {
                        self.ym2612.write_address_1(value);
                    }
                    0x01 => {
                        self.ym2612.write_data_1(value);
                    }
                    0x02 => {
                        self.ym2612.write_address_2(value);
                    }
                    0x03 => {
                        self.ym2612.write_data_2(value);
                    }
                    _ => unreachable!("value & 0x03 is always <= 0x03"),
                }
            }
            0x6000..=0x60FF => {
                self.memory.z80_bank_register.write_bit(value.bit(0));
            }
            0x6100..=0x7EFF | 0x7F20..=0x7FFF => {
                // Unused / invalid addresses
                // TODO writes to $7F20-$7FFF should halt the system
            }
            0x7F00..=0x7F1F => {
                // VDP addresses
                self.write_vdp_byte(address.into(), value);
            }
            0x8000..=0xFFFF => {
                let m68k_addr = self.memory.z80_bank_register.map_to_68k_address(address);
                if !(0xA00000..=0xA0FFFF).contains(&m68k_addr) {
                    <Self as m68000_emu::BusInterface>::write_byte(self, m68k_addr, value);
                } else {
                    // TODO this should lock up the system
                    panic!(
                        "Z80 attempted to read its own memory from the 68k bus; z80_addr={address:04X}, m68k_addr={m68k_addr:08X}"
                    );
                }
            }
        }
    }

    #[inline]
    fn read_io(&mut self, _address: u16) -> u8 {
        // I/O ports are not wired up to the Z80
        0xFF
    }

    #[inline]
    fn write_io(&mut self, _address: u16, _value: u8) {
        // I/O ports are not wired up to the Z80
    }

    #[inline]
    fn nmi(&self) -> InterruptLine {
        // The NMI line is not connected to anything
        InterruptLine::High
    }

    #[inline]
    fn int(&self) -> InterruptLine {
        self.vdp.z80_interrupt_line()
    }

    #[inline]
    fn busreq(&self) -> bool {
        self.memory.signals.z80_busreq
    }

    #[inline]
    fn reset(&self) -> bool {
        self.memory.signals.z80_reset
    }
}
