use bincode::{Decode, Encode};
use jgenesis_traits::num::GetBit;
use std::array;

const MAIN_RAM_LEN: usize = 128 * 1024;

type MainRam = [u8; MAIN_RAM_LEN];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CartridgeLocation {
    Rom(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Encode, Decode)]
enum Mapper {
    LoRom,
}

impl Mapper {
    fn guess_from_rom(_rom: &[u8]) -> Option<Self> {
        // TODO actually try to guess the mapper
        Some(Mapper::LoRom)
    }

    fn map_address(self, address: u32) -> CartridgeLocation {
        match self {
            Self::LoRom => {
                // TODO handle SRAM
                let rom_addr = ((address & 0xFF0000) >> 1) | (address & 0x007FFF);
                CartridgeLocation::Rom(rom_addr)
            }
        }
    }
}

#[derive(Debug, Clone, Encode, Decode)]
struct Cartridge {
    rom: Box<[u8]>,
    mapper: Mapper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum Memory2Speed {
    Fast,
    #[default]
    Slow,
}

impl Memory2Speed {
    fn from_byte(byte: u8) -> Self {
        if byte.bit(0) { Self::Fast } else { Self::Slow }
    }
}

#[derive(Debug, Clone, Encode, Decode)]
pub struct Memory {
    cartridge: Cartridge,
    main_ram: Box<MainRam>,
    wram_port_address: u32,
}

impl Memory {
    pub fn from_rom(rom: Vec<u8>) -> Self {
        let mapper = Mapper::guess_from_rom(&rom).expect("unable to determine mapper");
        let cartridge = Cartridge { rom: rom.into_boxed_slice(), mapper };

        Self {
            cartridge,
            main_ram: vec![0; MAIN_RAM_LEN].into_boxed_slice().try_into().unwrap(),
            wram_port_address: 0,
        }
    }

    pub fn read_cartridge(&mut self, address: u32) -> u8 {
        match self.cartridge.mapper.map_address(address) {
            CartridgeLocation::Rom(rom_addr) => {
                // TODO figure out mirroring for unusual ROM sizes
                self.cartridge.rom[(rom_addr as usize) % self.cartridge.rom.len()]
            }
        }
    }

    pub fn write_cartridge(&mut self, address: u32, value: u8) {
        todo!("write cartridge {address:06X} {value:02X}")
    }

    pub fn read_wram(&self, address: u32) -> u8 {
        self.main_ram[(address as usize) & (MAIN_RAM_LEN - 1)]
    }

    pub fn write_wram(&mut self, address: u32, value: u8) {
        self.main_ram[(address as usize) & (MAIN_RAM_LEN - 1)] = value;
    }

    pub fn read_wram_port(&mut self) -> u8 {
        let value = self.main_ram[self.wram_port_address as usize];
        self.increment_wram_port_address();
        value
    }

    pub fn write_wram_port(&mut self, value: u8) {
        self.main_ram[self.wram_port_address as usize] = value;
        self.increment_wram_port_address();
    }

    fn increment_wram_port_address(&mut self) {
        self.wram_port_address = (self.wram_port_address + 1) & ((MAIN_RAM_LEN - 1) as u32);
    }

    pub fn write_wram_port_address_low(&mut self, value: u8) {
        self.wram_port_address = (self.wram_port_address & 0xFFFF00) | u32::from(value);
        log::trace!("WRAM port address: {:06X}", self.wram_port_address);
    }

    pub fn write_wram_port_address_mid(&mut self, value: u8) {
        self.wram_port_address = (self.wram_port_address & 0xFF00FF) | (u32::from(value) << 8);
        log::trace!("WRAM port address: {:06X}", self.wram_port_address);
    }

    pub fn write_wram_port_address_high(&mut self, value: u8) {
        // Only 1 bit used from high byte
        self.wram_port_address =
            (self.wram_port_address & 0x00FFFF) | (u32::from(value & 0x01) << 16);
        log::trace!("WRAM port address: {:06X}", self.wram_port_address);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
enum IrqMode {
    // No IRQs
    #[default]
    Off,
    // IRQ at H=HTIME, every line
    H,
    // IRQ at V=VTIME + H=0
    V,
    // IRQ at V=VTIME + H=HTIME
    HV,
}

impl IrqMode {
    fn from_byte(byte: u8) -> Self {
        match byte & 0x30 {
            0x00 => Self::Off,
            0x10 => Self::H,
            0x20 => Self::V,
            0x30 => Self::HV,
            _ => unreachable!("value & 0x30 will always be one of the above values"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
enum DmaDirection {
    AtoB,
    #[default]
    BtoA,
}

impl DmaDirection {
    fn from_byte(byte: u8) -> Self {
        if byte.bit(7) { Self::BtoA } else { Self::AtoB }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
enum HdmaAddressingMode {
    Direct,
    #[default]
    Indirect,
}

impl HdmaAddressingMode {
    fn from_byte(byte: u8) -> Self {
        if byte.bit(6) { Self::Indirect } else { Self::Direct }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
enum DmaIncrementMode {
    #[default]
    Fixed,
    Increment,
    Decrement,
}

impl DmaIncrementMode {
    fn from_byte(byte: u8) -> Self {
        match byte & 0x18 {
            0x00 => Self::Increment,
            0x10 => Self::Decrement,
            0x08 | 0x18 => Self::Fixed,
            _ => unreachable!("value & 0x18 is always one of the above values"),
        }
    }
}

// Registers/ports that are on the 5A22 chip but are not part of the 65816
#[derive(Debug, Clone, Encode, Decode)]
pub struct CpuInternalRegisters {
    nmi_enabled: bool,
    irq_mode: IrqMode,
    auto_joypad_read_enabled: bool,
    irq_htime: u16,
    irq_vtime: u16,
    multiply_operand_l: u8,
    multiply_operand_r: u8,
    multiply_product: u16,
    division_dividend: u16,
    division_divisor: u8,
    division_quotient: u16,
    memory_2_speed: Memory2Speed,
    active_gpdma_channels: [bool; 8],
    dma_direction: [DmaDirection; 8],
    hdma_addressing_mode: [HdmaAddressingMode; 8],
    dma_increment_mode: [DmaIncrementMode; 8],
    dma_transfer_unit: [u8; 8],
    dma_bus_b_address: [u8; 8],
    // GPDMA current address is also used as HDMA table start address
    gpdma_current_address: [u16; 8],
    dma_bank: [u8; 8],
    // GPDMA byte counter is also used as HDMA indirect address
    gpdma_byte_counter: [u16; 8],
    hdma_indirect_bank: [u8; 8],
    hdma_table_current_address: [u16; 8],
    vblank_flag: bool,
    hblank_flag: bool,
    last_h: u16,
}

impl CpuInternalRegisters {
    pub fn new() -> Self {
        Self {
            nmi_enabled: false,
            irq_mode: IrqMode::default(),
            auto_joypad_read_enabled: false,
            irq_htime: 0,
            irq_vtime: 0,
            multiply_operand_l: 0xFF,
            multiply_operand_r: 0xFF,
            multiply_product: 0,
            division_dividend: 0xFFFF,
            division_divisor: 0xFF,
            division_quotient: 0,
            memory_2_speed: Memory2Speed::default(),
            active_gpdma_channels: [false; 8],
            dma_direction: [DmaDirection::default(); 8],
            hdma_addressing_mode: [HdmaAddressingMode::default(); 8],
            dma_increment_mode: [DmaIncrementMode::default(); 8],
            dma_transfer_unit: [0x07; 8],
            dma_bus_b_address: [0xFF; 8],
            gpdma_current_address: [0xFFFF; 8],
            dma_bank: [0xFF; 8],
            gpdma_byte_counter: [0xFFFF; 8],
            hdma_indirect_bank: [0xFF; 8],
            hdma_table_current_address: [0xFFFF; 8],
            vblank_flag: false,
            hblank_flag: false,
            last_h: 0,
        }
    }

    pub fn read_register(&mut self, address: u32) -> u8 {
        match address {
            0x4214 => {
                // RDDIVL: Division quotient, low byte
                self.division_quotient as u8
            }
            0x4215 => {
                // RDDIVH: Division quotient, high byte
                (self.division_quotient >> 8) as u8
            }
            0x4216 => {
                // RDMPYL: Multiply product / division remainder, low byte
                self.multiply_product as u8
            }
            0x4217 => {
                // RDMPYH: Multiply product / division remainder, high byte
                (self.multiply_product >> 8) as u8
            }
            _ => todo!("read register {address:06X}"),
        }
    }

    pub fn write_register(&mut self, address: u32, value: u8) {
        log::trace!("CPU internal register write: {address:06X} {value:02X}");

        match address & 0xFFFF {
            0x4016 => {
                // JOYWR: Joypad output
                // TODO handle strobe in bit 0

                log::warn!("Unhandled JOYWR write: {value:02X}");
            }
            0x4200 => {
                // NMITIMEN: Interrupt enable and joypad request
                self.auto_joypad_read_enabled = value.bit(0);
                self.irq_mode = IrqMode::from_byte(value);
                self.nmi_enabled = value.bit(7);

                log::trace!("  Auto joypad read enabled: {}", self.auto_joypad_read_enabled);
                log::trace!("  IRQ mode: {:?}", self.irq_mode);
                log::trace!("  NMI enabled: {}", self.nmi_enabled);
            }
            0x4201 => {
                // WRIO: Joypad programmable I/O port (write)
                // TODO implement this?

                log::warn!("Unhandled WRIO write: {value:02X}");
            }
            0x4202 => {
                // WRMPYA: Multiplication 8-bit operand A
                self.multiply_operand_l = value;

                log::trace!("  Unsigned multiply operand A: {value:02X}");
            }
            0x4203 => {
                // WRMPYB: Multiplication 8-bit operand B + start multiplication
                self.multiply_operand_r = value;

                // TODO delay setting the result? takes 8 CPU cycles on real hardware
                self.multiply_product = u16::from(self.multiply_operand_l) * u16::from(value);

                // Multiplication always writes operand B to the division quotient register
                self.division_quotient = value.into();

                log::trace!("  Unsigned multiply operand B: {value:02X}");
                log::trace!("  Unsigned multiply product: {:04X}", self.multiply_product);
            }
            0x4204 => {
                // WRDIVL: Division 16-bit dividend, low byte
                self.division_dividend = (self.division_dividend & 0xFF00) | u16::from(value);

                log::trace!("  Unsigned divide dividend: {:04X}", self.division_dividend);
            }
            0x4205 => {
                // WRDIVH: Division 16-bit dividend, high byte
                self.division_dividend =
                    (self.division_dividend & 0x00FF) | (u16::from(value) << 8);

                log::trace!("  Unsigned divide dividend: {:04X}", self.division_dividend);
            }
            0x4206 => {
                // WRDIVB: Division 8-bit divisor + start division
                self.division_divisor = value;

                // TODO delay setting the result? takes 16 CPU cycles on real hardware
                if value != 0 {
                    self.division_quotient = self.division_dividend / u16::from(value);

                    // Division writes remainder to the multiply product register
                    self.multiply_product = self.division_dividend % u16::from(value);
                } else {
                    // Divide by 0 always sets quotient to $FFFF and remainder to dividend
                    self.division_quotient = 0xFFFF;
                    self.multiply_product = self.division_dividend;
                }

                log::trace!("  Unsigned divide divisor: {value:02X}");
                log::trace!("  Unsigned divide quotient: {:04X}", self.division_quotient);
                log::trace!("  Unsigned divide remainder: {:04X}", self.multiply_product);
            }
            0x4207 => {
                // HTIMEL: H-count timer setting, low byte
                self.irq_htime = (self.irq_htime & 0xFF00) | u16::from(value);

                log::trace!("  HTIME: {:04X}", self.irq_htime);
            }
            0x4208 => {
                // HTIMEH: H-count timer setting, high byte (really just highest bit)
                self.irq_htime = (self.irq_htime & 0x00FF) | (u16::from(value & 0x01) << 8);

                log::trace!("  HTIME: {:04X}", self.irq_htime);
            }
            0x4209 => {
                // VTIMEL: V-count timer setting, low byte
                self.irq_vtime = (self.irq_vtime & 0xFF00) | u16::from(value);

                log::trace!("  VTIME: {:04X}", self.irq_vtime);
            }
            0x420A => {
                // VTIMEH: V-count timer setting, high byte (really just highest bit)
                self.irq_vtime = (self.irq_vtime & 0x00FF) | (u16::from(value & 0x01) << 8);

                log::trace!("  VTIME: {:04X}", self.irq_vtime);
            }
            0x420B => {
                // MDMAEN: Select general purpose DMA channels + start transfer (if non-zero)
                self.active_gpdma_channels = array::from_fn(|i| value.bit(i as u8));

                log::trace!("  GPDMA active channels: {value:02X}");
            }
            0x420C => {
                // HDMAEN: Select HBlank DMA channels + start transfer
                if value != 0 {
                    todo!("HDMA: {value:02X}")
                }
            }
            0x420D => {
                // MEMSEL: Memory-2 waitstate control
                self.memory_2_speed = Memory2Speed::from_byte(value);

                log::trace!("  Memory-2 speed: {:?}", self.memory_2_speed);
            }
            address @ 0x4300..=0x437F => {
                // DMA registers
                self.write_dma_register(address, value);
            }
            _ => todo!("write register {address:06X} {value:02X}"),
        }
    }

    fn write_dma_register(&mut self, address: u32, value: u8) {
        // Second-least significant nibble is channel
        let channel = ((address >> 4) & 0x7) as usize;

        log::trace!("  DMA channel: {channel}");

        match address & 0xFF0F {
            0x4300 => {
                // DMAPx: DMA parameters 0-7
                self.dma_transfer_unit[channel] = value & 0x07;
                self.dma_increment_mode[channel] = DmaIncrementMode::from_byte(value);
                self.hdma_addressing_mode[channel] = HdmaAddressingMode::from_byte(value);
                self.dma_direction[channel] = DmaDirection::from_byte(value);

                log::trace!("  DMA transfer unit: {}", self.dma_transfer_unit[channel]);
                log::trace!("  DMA increment mode: {:?}", self.dma_increment_mode[channel]);
                log::trace!("  HDMA addressing mode: {:?}", self.hdma_addressing_mode[channel]);
                log::trace!("  DMA direction: {:?}", self.dma_direction[channel]);
            }
            0x4301 => {
                // BBADx: DMA bus B address
                self.dma_bus_b_address[channel] = value;

                log::trace!("  DMA bus B address: {value:02X}");
            }
            0x4302 => {
                // A1TxL: GPDMA current address / HDMA table start address, low byte
                self.gpdma_current_address[channel] =
                    (self.gpdma_current_address[channel] & 0xFF00) | u16::from(value);

                log::trace!(
                    "  GPDMA current address / HDMA table start address: {:04X}",
                    self.gpdma_current_address[channel]
                );
            }
            0x4303 => {
                // A1TxH: GPDMA current address / HDMA table start address, high byte
                self.gpdma_current_address[channel] =
                    (self.gpdma_current_address[channel] & 0x00FF) | (u16::from(value) << 8);

                log::trace!(
                    "  GPDMA current address / HDMA table start address: {:04X}",
                    self.gpdma_current_address[channel]
                );
            }
            0x4304 => {
                // A1Bx: GPDMA current address / HDMA table start address, bank
                self.dma_bank[channel] = value;

                log::trace!(
                    "  GPDMA current address bank / HDMA table start address bank: {value:02X}"
                );
            }
            0x4305 => {
                // DASxL: GPDMA byte counter / HDMA indirect address, low byte
                self.gpdma_byte_counter[channel] =
                    (self.gpdma_byte_counter[channel] & 0xFF00) | u16::from(value);

                log::trace!(
                    "  GPDMA byte counter / HDMA indirect address: {:04X}",
                    self.gpdma_byte_counter[channel]
                );
            }
            0x4306 => {
                // DASxH: GPDMA byte counter / HDMA indirect address, high byte
                self.gpdma_byte_counter[channel] =
                    (self.gpdma_byte_counter[channel] & 0x00FF) | (u16::from(value) << 8);

                log::trace!(
                    "  GPDMA byte counter / HDMA indirect address: {:04X}",
                    self.gpdma_byte_counter[channel]
                );
            }
            0x4307 => {
                // DASBx: HDMA indirect address, bank
                self.hdma_indirect_bank[channel] = value;

                log::trace!("  HDMA indirect address bank: {value:02X}");
            }
            0x4308 => {
                // A2AxL: HDMA table current address, low byte
                self.hdma_table_current_address[channel] =
                    (self.hdma_table_current_address[channel] & 0xFF00) | u16::from(value);

                log::trace!(
                    "  HDMA table current address: {:04X}",
                    self.hdma_table_current_address[channel]
                );
            }
            0x4309 => {
                // A2AxH: HDMA table current address, high byte
                self.hdma_table_current_address[channel] =
                    (self.hdma_table_current_address[channel] & 0x00FF) | (u16::from(value) << 8);

                log::trace!(
                    "  HDMA table current address: {:04X}",
                    self.hdma_table_current_address[channel]
                );
            }
            _ => todo!("write DMA register {address:06X} {value:02X}"),
        }
    }

    pub fn memory_2_speed(&self) -> Memory2Speed {
        self.memory_2_speed
    }
}
