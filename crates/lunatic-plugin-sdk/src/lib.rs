#![forbid(unsafe_code)]

use anyhow::{Result, anyhow};

/// WebAssembly value types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValType {
    I32,
    I64,
    F32,
    F64,
    V128,
    FuncRef,
    ExternRef,
}

impl ValType {
    /// Convert to the WebAssembly binary encoding byte
    pub fn to_byte(self) -> u8 {
        match self {
            ValType::I32 => 0x7F,
            ValType::I64 => 0x7E,
            ValType::F32 => 0x7D,
            ValType::F64 => 0x7C,
            ValType::V128 => 0x7B,
            ValType::FuncRef => 0x70,
            ValType::ExternRef => 0x6F,
        }
    }

    /// Parse from a WebAssembly binary encoding byte
    pub fn from_byte(byte: u8) -> Result<Self> {
        match byte {
            0x7F => Ok(ValType::I32),
            0x7E => Ok(ValType::I64),
            0x7D => Ok(ValType::F32),
            0x7C => Ok(ValType::F64),
            0x7B => Ok(ValType::V128),
            0x70 => Ok(ValType::FuncRef),
            0x6F => Ok(ValType::ExternRef),
            _ => Err(anyhow!("Unknown WebAssembly value type: 0x{byte:02X}")),
        }
    }
}

/// A type-safe index into the module's type section
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeIndex(pub u32);

/// A type-safe index into the module's function section
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FuncIndex(pub u32);

/// A WebAssembly function type (signature)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionType {
    pub params: Vec<ValType>,
    pub returns: Vec<ValType>,
}

impl FunctionType {
    pub fn new(params: Vec<ValType>, returns: Vec<ValType>) -> Self {
        Self { params, returns }
    }
}

/// Local variable declaration for a function
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    pub count: u32,
    pub val_type: ValType,
}

impl Local {
    pub fn new(count: u32, val_type: ValType) -> Self {
        Self { count, val_type }
    }

    /// Encode locals to the binary format expected by ModuleContext::add_function
    /// Format: 4 bytes (LE u32 count) + 1 byte (type)
    pub fn encode(&self) -> [u8; 5] {
        let count_bytes = self.count.to_le_bytes();
        [
            count_bytes[0],
            count_bytes[1],
            count_bytes[2],
            count_bytes[3],
            self.val_type.to_byte(),
        ]
    }
}

/// Encode a slice of locals into the binary format
pub fn encode_locals(locals: &[Local]) -> Vec<u8> {
    locals.iter().flat_map(|l| l.encode()).collect()
}

/// Builder for constructing plugin modifications to a module
#[derive(Debug, Default)]
pub struct PluginBuilder {
    types: Vec<FunctionType>,
    functions: Vec<(TypeIndex, Vec<Local>, Vec<u8>)>,
    exports: Vec<(String, FuncIndex)>,
}

impl PluginBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a function type (signature) to the module
    pub fn add_function_type(&mut self, func_type: FunctionType) -> TypeIndex {
        let idx = TypeIndex(self.types.len() as u32);
        self.types.push(func_type);
        idx
    }

    /// Add a function to the module with the given type, locals, and body bytecode
    pub fn add_function(
        &mut self,
        type_idx: TypeIndex,
        locals: Vec<Local>,
        body: Vec<u8>,
    ) -> FuncIndex {
        let idx = FuncIndex(self.functions.len() as u32);
        self.functions.push((type_idx, locals, body));
        idx
    }

    /// Export a function by name
    pub fn add_function_export(&mut self, name: impl Into<String>, func_idx: FuncIndex) {
        self.exports.push((name.into(), func_idx));
    }

    /// Get the types added so far
    pub fn types(&self) -> &[FunctionType] {
        &self.types
    }

    /// Get the functions added so far
    pub fn functions(&self) -> &[(TypeIndex, Vec<Local>, Vec<u8>)] {
        &self.functions
    }

    /// Get the exports added so far
    pub fn exports(&self) -> &[(String, FuncIndex)] {
        &self.exports
    }
}

/// Encode a u32 value as a LEB128 byte sequence
pub fn encode_leb128_u32(mut value: u32) -> Vec<u8> {
    let mut result = Vec::new();
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        result.push(byte);
        if value == 0 {
            break;
        }
    }
    result
}

/// Encode an i32 value as a signed LEB128 byte sequence
pub fn encode_leb128_i32(mut value: i32) -> Vec<u8> {
    let mut result = Vec::new();
    let mut more = true;
    while more {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if (value == 0 && (byte & 0x40) == 0) || (value == -1 && (byte & 0x40) != 0) {
            more = false;
        } else {
            byte |= 0x80;
        }
        result.push(byte);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_val_type_roundtrip() {
        for ty in [
            ValType::I32,
            ValType::I64,
            ValType::F32,
            ValType::F64,
            ValType::V128,
            ValType::FuncRef,
            ValType::ExternRef,
        ] {
            assert_eq!(ValType::from_byte(ty.to_byte()).unwrap(), ty);
        }
    }

    #[test]
    fn test_val_type_invalid_byte() {
        assert!(ValType::from_byte(0x00).is_err());
        assert!(ValType::from_byte(0xFF).is_err());
    }

    #[test]
    fn test_local_encode() {
        let local = Local::new(3, ValType::I32);
        let encoded = local.encode();
        assert_eq!(encoded, [3, 0, 0, 0, 0x7F]);
    }

    #[test]
    fn test_encode_locals() {
        let locals = vec![Local::new(1, ValType::I32), Local::new(2, ValType::I64)];
        let encoded = encode_locals(&locals);
        assert_eq!(encoded.len(), 10);
        assert_eq!(&encoded[0..5], &[1, 0, 0, 0, 0x7F]);
        assert_eq!(&encoded[5..10], &[2, 0, 0, 0, 0x7E]);
    }

    #[test]
    fn test_plugin_builder() {
        let mut builder = PluginBuilder::new();

        let type_idx =
            builder.add_function_type(FunctionType::new(vec![ValType::I32], vec![ValType::I32]));
        assert_eq!(type_idx, TypeIndex(0));

        let func_idx = builder.add_function(
            type_idx,
            vec![Local::new(1, ValType::I32)],
            vec![0x20, 0x00, 0x0B], // local.get 0, end
        );
        assert_eq!(func_idx, FuncIndex(0));

        builder.add_function_export("my_func", func_idx);

        assert_eq!(builder.types().len(), 1);
        assert_eq!(builder.functions().len(), 1);
        assert_eq!(builder.exports().len(), 1);
    }

    #[test]
    fn test_leb128_u32() {
        assert_eq!(encode_leb128_u32(0), vec![0x00]);
        assert_eq!(encode_leb128_u32(1), vec![0x01]);
        assert_eq!(encode_leb128_u32(127), vec![0x7F]);
        assert_eq!(encode_leb128_u32(128), vec![0x80, 0x01]);
        assert_eq!(encode_leb128_u32(624485), vec![0xE5, 0x8E, 0x26]);
    }

    #[test]
    fn test_leb128_i32() {
        assert_eq!(encode_leb128_i32(0), vec![0x00]);
        assert_eq!(encode_leb128_i32(1), vec![0x01]);
        assert_eq!(encode_leb128_i32(-1), vec![0x7F]);
        assert_eq!(encode_leb128_i32(-128), vec![0x80, 0x7F]);
    }

    #[test]
    fn test_function_type() {
        let ft = FunctionType::new(vec![ValType::I32, ValType::I64], vec![ValType::F32]);
        assert_eq!(ft.params.len(), 2);
        assert_eq!(ft.returns.len(), 1);
    }
}
