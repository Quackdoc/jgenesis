mod arithmetic;
mod bits;
mod load;

use crate::core::{
    AddressRegister, AddressingMode, DataRegister, Exception, ExecuteResult, InstructionExecutor,
    OpSize,
};
use crate::traits::{BusInterface, GetBit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    RegisterToMemory,
    MemoryToRegister,
}

impl Direction {
    fn parse_from_opcode(opcode: u16) -> Self {
        if opcode.bit(8) {
            Self::RegisterToMemory
        } else {
            Self::MemoryToRegister
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UspDirection {
    RegisterToUsp,
    UspToRegister,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtendOpMode {
    DataDirect,
    AddressIndirectPredecrement,
}

impl ExtendOpMode {
    fn parse_from_opcode(opcode: u16) -> Self {
        if opcode.bit(3) {
            Self::AddressIndirectPredecrement
        } else {
            Self::DataDirect
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instruction {
    Add {
        size: OpSize,
        source: AddressingMode,
        dest: AddressingMode,
    },
    AddExtend {
        size: OpSize,
        source: AddressingMode,
        dest: AddressingMode,
    },
    And {
        size: OpSize,
        source: AddressingMode,
        dest: AddressingMode,
    },
    AndToCcr,
    AndToSr,
    ExclusiveOr {
        size: OpSize,
        source: AddressingMode,
        dest: AddressingMode,
    },
    ExclusiveOrToCcr,
    ExclusiveOrToSr,
    Move {
        size: OpSize,
        source: AddressingMode,
        dest: AddressingMode,
    },
    MoveFromSr(AddressingMode),
    MoveToCcr(AddressingMode),
    MoveToSr(AddressingMode),
    MoveUsp(UspDirection, AddressRegister),
    MoveQuick(i8, DataRegister),
    Or {
        size: OpSize,
        source: AddressingMode,
        dest: AddressingMode,
    },
    OrToCcr,
    OrToSr,
}

impl Instruction {
    pub fn source_addressing_mode(self) -> Option<AddressingMode> {
        match self {
            Self::Add { source, .. }
            | Self::AddExtend { source, .. }
            | Self::And { source, .. }
            | Self::ExclusiveOr { source, .. }
            | Self::Move { source, .. }
            | Self::MoveToCcr(source)
            | Self::MoveToSr(source)
            | Self::Or { source, .. } => Some(source),
            Self::AndToCcr
            | Self::AndToSr
            | Self::ExclusiveOrToCcr
            | Self::ExclusiveOrToSr
            | Self::MoveQuick(..)
            | Self::MoveFromSr(..)
            | Self::MoveUsp(..)
            | Self::OrToCcr
            | Self::OrToSr => None,
        }
    }

    pub fn dest_addressing_mode(self) -> Option<AddressingMode> {
        match self {
            Self::Add { dest, .. }
            | Self::AddExtend { dest, .. }
            | Self::And { dest, .. }
            | Self::ExclusiveOr { dest, .. }
            | Self::Move { dest, .. }
            | Self::MoveFromSr(dest)
            | Self::Or { dest, .. } => Some(dest),
            Self::AndToCcr
            | Self::AndToSr
            | Self::ExclusiveOrToCcr
            | Self::ExclusiveOrToSr
            | Self::MoveToCcr(..)
            | Self::MoveToSr(..)
            | Self::MoveUsp(..)
            | Self::MoveQuick(..)
            | Self::OrToCcr
            | Self::OrToSr => None,
        }
    }
}

impl<'registers, 'bus, B: BusInterface> InstructionExecutor<'registers, 'bus, B> {
    pub(super) fn do_execute(&mut self) -> ExecuteResult<()> {
        let opcode = self.fetch_operand()?;
        self.opcode = opcode;

        let instruction = decode_opcode(opcode, self.registers.supervisor_mode)?;
        self.instruction = Some(instruction);
        log::trace!("Decoded instruction: {instruction:?}");

        match instruction {
            Instruction::Add { size, source, dest } => self.add(size, source, dest),
            Instruction::AddExtend { size, source, dest } => self.addx(size, source, dest),
            Instruction::And { size, source, dest } => self.and(size, source, dest),
            Instruction::AndToCcr => self.andi_to_ccr(),
            Instruction::AndToSr => self.andi_to_sr(),
            Instruction::ExclusiveOr { size, source, dest } => self.eor(size, source, dest),
            Instruction::ExclusiveOrToCcr => self.eori_to_ccr(),
            Instruction::ExclusiveOrToSr => self.eori_to_sr(),
            Instruction::Move { size, source, dest } => self.move_(size, source, dest),
            Instruction::MoveFromSr(dest) => self.move_from_sr(dest),
            Instruction::MoveToCcr(source) => self.move_to_ccr(source),
            Instruction::MoveToSr(source) => self.move_to_sr(source),
            Instruction::MoveQuick(data, register) => {
                self.moveq(data, register);
                Ok(())
            }
            Instruction::MoveUsp(direction, register) => {
                self.move_usp(direction, register);
                Ok(())
            }
            Instruction::Or { size, source, dest } => self.or(size, source, dest),
            Instruction::OrToCcr => self.ori_to_ccr(),
            Instruction::OrToSr => self.ori_to_sr(),
        }
    }
}

fn decode_opcode(opcode: u16, supervisor_mode: bool) -> ExecuteResult<Instruction> {
    match opcode & 0xF000 {
        0x0000 => match opcode & 0b0000_1111_0000_0000 {
            0b0000_0000_0000_0000 => bits::decode_ori(opcode, supervisor_mode),
            0b0000_0010_0000_0000 => bits::decode_andi(opcode, supervisor_mode),
            0b0000_0100_0000_0000 => todo!("SUBI"),
            0b0000_0110_0000_0000 => arithmetic::decode_addi(opcode),
            0b0000_1010_0000_0000 => bits::decode_eori(opcode, supervisor_mode),
            0b0000_1100_0000_0000 => todo!("CMPI"),
            0b0000_1000_0000_0000 => todo!("BTST / BCHG / BCLR / BSET (immediate)"),
            _ => {
                if opcode.bit(8) {
                    todo!("BTST / BCHG / BCLR / BSET (data register")
                } else {
                    Err(Exception::IllegalInstruction(opcode))
                }
            }
        },
        0x1000 | 0x2000 | 0x3000 => load::decode_move(opcode),
        0x4000 => match opcode & 0b0000_1111_1100_0000 {
            0b0000_0000_1100_0000 => load::decode_move_from_sr(opcode),
            0b0000_0100_1100_0000 => load::decode_move_to_ccr(opcode),
            0b0000_0110_1100_0000 => load::decode_move_to_sr(opcode, supervisor_mode),
            0b0000_0000_0000_0000 | 0b0000_0000_0100_0000 | 0b0000_0000_1000_0000 => todo!("NEGX"),
            0b0000_0010_0000_0000 | 0b0000_0010_0100_0000 | 0b0000_0010_1000_0000 => todo!("CLR"),
            0b0000_0100_0000_0000 | 0b0000_0100_0100_0000 | 0b0000_0100_1000_0000 => todo!("NEG"),
            0b0000_0110_0000_0000 | 0b0000_0110_0100_0000 | 0b0000_0110_1000_0000 => todo!("NOT"),
            0b0000_1000_1000_0000
            | 0b0000_1000_1100_0000
            | 0b0000_1100_1000_0000
            | 0b0000_1100_1100_0000 => todo!("EXT / MOVEM"),
            0b0000_1000_0000_0000 => todo!("NBCD"),
            0b0000_1000_0100_0000 => todo!("SWAP / PEA"),
            0b0000_1010_1100_0000 => todo!("ILLEGAL / 0TAS"),
            0b0000_1010_0000_0000 | 0b0000_1010_0100_0000 | 0b0000_1010_1000_0000 => todo!("TST"),
            0b0000_1110_0100_0000 => match opcode & 0b0000_0000_0011_1111 {
                0b0000_0000_0011_0000 => todo!("RESET"),
                0b0000_0000_0011_0001 => todo!("NOP"),
                0b0000_0000_0011_0010 => todo!("STOP"),
                _ => match opcode & 0b0000_0000_0011_1000 {
                    0b0000_0000_0000_0000 | 0b0000_0000_0000_1000 => todo!("TRAP"),
                    0b0000_0000_0001_0000 => todo!("LINK"),
                    0b0000_0000_0001_1000 => todo!("UNLK"),
                    0b0000_0000_0010_0000 | 0b0000_0000_0010_1000 => {
                        load::decode_move_usp(opcode, supervisor_mode)
                    }
                    _ => Err(Exception::IllegalInstruction(opcode)),
                },
            },
            0b0000_1110_1000_0000 => todo!("JSR"),
            0b0000_1110_1100_0000 => todo!("JMP"),
            _ => todo!("LEA / CHK"),
        },
        0x5000 => match OpSize::parse_from_opcode(opcode) {
            Ok(size) => arithmetic::decode_addq_subq(opcode, size),
            Err(_) => {
                todo!("Scc / DBcc")
            }
        },
        0x6000 => todo!("BRA / BSR / Bcc"),
        0x7000 => load::decode_movq(opcode),
        0x8000 => match opcode & 0b0000_0001_1111_0000 {
            0b0000_0001_0000_0000 => todo!("SBCD"),
            _ => match opcode & 0b0000_0001_1100_0000 {
                0b0000_0000_1100_0000 => todo!("DIVU"),
                0b0000_0001_1100_0000 => todo!("DIVS"),
                _ => bits::decode_or(opcode),
            },
        },
        0x9000 => todo!("SUB / SUBX / SUBA"),
        0xB000 => match opcode & 0b0000_0000_1100_0000 {
            0b0000_0000_1100_0000 => todo!("CMPA"),
            _ => {
                if opcode.bit(8) {
                    match opcode & 0b0000_0000_0011_1000 {
                        0b0000_0000_0000_1000 => todo!("CMPM"),
                        _ => bits::decode_eor(opcode),
                    }
                } else {
                    todo!("CMP")
                }
            }
        },
        0xC000 => {
            // AND (TODO: MULU / MULS / ABCD / EXG)
            bits::decode_and(opcode)
        }
        0xD000 => match opcode & 0b0000_0001_1111_0000 {
            0b0000_0001_0000_0000 | 0b0000_0001_0100_0000 | 0b0000_0001_1000_0000 => {
                arithmetic::decode_addx(opcode)
            }
            _ => arithmetic::decode_add(opcode),
        },
        0xE000 => todo!("ASd / LSd / ROXd / ROd"),
        _ => Err(Exception::IllegalInstruction(opcode)),
    }
}