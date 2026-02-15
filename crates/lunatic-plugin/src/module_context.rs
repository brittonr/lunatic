use anyhow::{anyhow, Result};
use wasmparser::{Parser, Payload};

/// Represents a type that was either parsed from the original module or newly added
enum ContextType {
    /// A newly added function type (params, returns)
    New(Vec<wasm_encoder::ValType>, Vec<wasm_encoder::ValType>),
}

/// Represents a function body
enum ContextCode {
    /// A function with locals as (count, type) pairs and body bytes
    New(Vec<(u32, wasm_encoder::ValType)>, Vec<u8>),
}

/// Represents an export
enum ContextExport {
    /// A newly added function export (name, function index)
    NewFunction(String, u32),
    /// An export parsed from the original module
    Parsed {
        name: String,
        kind: wasmparser::ExternalKind,
        index: u32,
    },
}

/// A raw section from the original module that we preserve as-is
struct RawSection {
    id: u8,
    data: Vec<u8>,
}

/// Parsed import stored structurally for re-encoding
struct ParsedImport {
    module: String,
    name: String,
    ty: wasm_encoder::EntityType,
}

/// Context for manipulating a WebAssembly module.
///
/// Parses an existing module and allows adding new types, functions, and exports
/// before re-encoding the module. This enables plugins to transform modules
/// without unsafe code.
pub struct ModuleContext {
    types: Vec<ContextType>,
    functions: Vec<u32>,
    code_section: Vec<ContextCode>,
    imports: Vec<ParsedImport>,
    import_func_count: u32,
    exports: Vec<ContextExport>,
    sections: Vec<RawSection>,
    function_names: std::collections::HashMap<String, u32>,
}

impl ModuleContext {
    /// Parse a WebAssembly module binary into a ModuleContext
    pub fn new(module: &[u8]) -> Result<Self> {
        let mut types = Vec::new();
        let mut functions = Vec::new();
        let mut code_section = Vec::new();
        let mut imports = Vec::new();
        let mut import_func_count: u32 = 0;
        let mut exports = Vec::new();
        let mut sections = Vec::new();
        let mut function_names = std::collections::HashMap::new();

        let parser = Parser::new(0);
        for payload in parser.parse_all(module) {
            let payload = payload?;
            match payload {
                Payload::TypeSection(reader) => {
                    for rec_group in reader {
                        let rec_group = rec_group?;
                        for sub_type in rec_group.into_types() {
                            match &sub_type.composite_type.inner {
                                wasmparser::CompositeInnerType::Func(func_type) => {
                                    let params: Vec<wasm_encoder::ValType> = func_type
                                        .params()
                                        .iter()
                                        .map(|t| translate_val_type(*t))
                                        .collect::<Result<_>>()?;
                                    let returns: Vec<wasm_encoder::ValType> = func_type
                                        .results()
                                        .iter()
                                        .map(|t| translate_val_type(*t))
                                        .collect::<Result<_>>()?;
                                    types.push(ContextType::New(params, returns));
                                }
                                _ => {
                                    // TODO: Handle struct/array/cont types if needed
                                    return Err(anyhow!("Unsupported composite type in module"));
                                }
                            }
                        }
                    }
                }
                Payload::ImportSection(reader) => {
                    for import in reader {
                        let import = import?;
                        if matches!(import.ty, wasmparser::TypeRef::Func(_)) {
                            import_func_count += 1;
                        }
                        imports.push(translate_import(&import)?);
                    }
                }
                Payload::FunctionSection(reader) => {
                    for func in reader {
                        let type_idx = func?;
                        functions.push(type_idx);
                    }
                }
                Payload::ExportSection(reader) => {
                    for export in reader {
                        let export = export?;
                        if let wasmparser::ExternalKind::Func = export.kind {
                            function_names.insert(export.name.to_string(), export.index);
                        }
                        exports.push(ContextExport::Parsed {
                            name: export.name.to_string(),
                            kind: export.kind,
                            index: export.index,
                        });
                    }
                }
                Payload::CodeSectionEntry(body) => {
                    let mut locals = Vec::new();
                    let locals_reader = body.get_locals_reader()?;
                    for local in locals_reader {
                        let (count, val_type) = local?;
                        locals.push((count, translate_val_type(val_type)?));
                    }
                    let operators_reader = body.get_operators_reader()?;
                    let op_start = operators_reader.original_position();
                    let body_end = body.range().end;
                    let body_bytes = module[op_start..body_end].to_vec();
                    code_section.push(ContextCode::New(locals, body_bytes));
                }
                Payload::TableSection(reader) => {
                    let range = reader.range();
                    sections.push(RawSection {
                        id: 4,
                        data: module[range.start..range.end].to_vec(),
                    });
                }
                Payload::MemorySection(reader) => {
                    let range = reader.range();
                    sections.push(RawSection {
                        id: 5,
                        data: module[range.start..range.end].to_vec(),
                    });
                }
                Payload::GlobalSection(reader) => {
                    let range = reader.range();
                    sections.push(RawSection {
                        id: 6,
                        data: module[range.start..range.end].to_vec(),
                    });
                }
                Payload::StartSection { range, .. } => {
                    sections.push(RawSection {
                        id: 8,
                        data: module[range.start..range.end].to_vec(),
                    });
                }
                Payload::ElementSection(reader) => {
                    let range = reader.range();
                    sections.push(RawSection {
                        id: 9,
                        data: module[range.start..range.end].to_vec(),
                    });
                }
                Payload::DataSection(reader) => {
                    let range = reader.range();
                    sections.push(RawSection {
                        id: 11,
                        data: module[range.start..range.end].to_vec(),
                    });
                }
                Payload::DataCountSection { range, .. } => {
                    sections.push(RawSection {
                        id: 12,
                        data: module[range.start..range.end].to_vec(),
                    });
                }
                Payload::CustomSection(custom) => {
                    // TODO: Parse name section for function names
                    let range = custom.range();
                    sections.push(RawSection {
                        id: 0,
                        data: module[range.start..range.end].to_vec(),
                    });
                }
                _ => {
                    // Skip other payloads (version, end, code section start, etc.)
                }
            }
        }

        Ok(Self {
            types,
            functions,
            code_section,
            imports,
            import_func_count,
            exports,
            sections,
            function_names,
        })
    }

    /// Add a new function type (signature) to the module.
    /// Returns the type index.
    pub fn add_function_type(
        &mut self,
        params: Vec<wasm_encoder::ValType>,
        returns: Vec<wasm_encoder::ValType>,
    ) -> u32 {
        let idx = self.types.len() as u32;
        self.types.push(ContextType::New(params, returns));
        idx
    }

    /// Add a new function to the module.
    /// Returns the function index (accounting for imported functions).
    pub fn add_function(
        &mut self,
        type_index: u32,
        locals: Vec<(u32, wasm_encoder::ValType)>,
        body: Vec<u8>,
    ) -> u32 {
        let func_idx = self.import_func_count + self.functions.len() as u32;
        self.functions.push(type_index);
        self.code_section.push(ContextCode::New(locals, body));
        func_idx
    }

    /// Export a function by name
    pub fn add_function_export(&mut self, name: String, func_idx: u32) {
        self.exports
            .push(ContextExport::NewFunction(name, func_idx));
    }

    /// Look up a function index by its export name
    pub fn function_by_name(&self, name: &str) -> Option<u32> {
        self.function_names.get(name).copied()
    }

    /// Encode the (possibly modified) module back to WebAssembly binary format
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut module = wasm_encoder::Module::new();
        let mut section_iter = self.sections.iter().peekable();

        // Type section
        if !self.types.is_empty() {
            let mut type_section = wasm_encoder::TypeSection::new();
            for ty in &self.types {
                match ty {
                    ContextType::New(params, returns) => {
                        type_section.ty().function(
                            params.iter().copied(),
                            returns.iter().copied(),
                        );
                    }
                }
            }
            module.section(&type_section);
        }

        // Import section
        if !self.imports.is_empty() {
            let mut import_section = wasm_encoder::ImportSection::new();
            for imp in &self.imports {
                import_section.import(&imp.module, &imp.name, imp.ty);
            }
            module.section(&import_section);
        }

        // Function section
        if !self.functions.is_empty() {
            let mut func_section = wasm_encoder::FunctionSection::new();
            for &type_idx in &self.functions {
                func_section.function(type_idx);
            }
            module.section(&func_section);
        }

        // Table (4), Memory (5), Global (6) sections
        while section_iter.peek().is_some_and(|s| matches!(s.id, 4..=6)) {
            let section = section_iter.next().unwrap();
            module.section(&wasm_encoder::RawSection {
                id: section.id,
                data: &section.data,
            });
        }

        // Export section
        if !self.exports.is_empty() {
            let mut export_section = wasm_encoder::ExportSection::new();
            for export in &self.exports {
                match export {
                    ContextExport::NewFunction(name, idx) => {
                        export_section.export(name, wasm_encoder::ExportKind::Func, *idx);
                    }
                    ContextExport::Parsed { name, kind, index } => {
                        let enc_kind = translate_export_kind(*kind)?;
                        export_section.export(name, enc_kind, *index);
                    }
                }
            }
            module.section(&export_section);
        }

        // Start section (8)
        while section_iter.peek().is_some_and(|s| s.id == 8) {
            let section = section_iter.next().unwrap();
            module.section(&wasm_encoder::RawSection {
                id: section.id,
                data: &section.data,
            });
        }

        // Element section (9)
        while section_iter.peek().is_some_and(|s| s.id == 9) {
            let section = section_iter.next().unwrap();
            module.section(&wasm_encoder::RawSection {
                id: section.id,
                data: &section.data,
            });
        }

        // DataCount section (12) - must come before code section
        // We need to peek ahead for this since it may not be next
        let mut deferred_sections = Vec::new();
        while section_iter.peek().is_some_and(|s| !matches!(s.id, 0 | 11 | 12)) {
            deferred_sections.push(section_iter.next().unwrap());
        }

        // Emit DataCount if present
        while section_iter.peek().is_some_and(|s| s.id == 12) {
            let section = section_iter.next().unwrap();
            module.section(&wasm_encoder::RawSection {
                id: section.id,
                data: &section.data,
            });
        }

        // Code section
        if !self.code_section.is_empty() {
            let mut code_section = wasm_encoder::CodeSection::new();
            for code in &self.code_section {
                match code {
                    ContextCode::New(locals, body) => {
                        let mut func = wasm_encoder::Function::new(locals.iter().copied());
                        func.raw(body.iter().copied());
                        code_section.function(&func);
                    }
                }
            }
            module.section(&code_section);
        }

        // Data section (11)
        while section_iter.peek().is_some_and(|s| s.id == 11) {
            let section = section_iter.next().unwrap();
            module.section(&wasm_encoder::RawSection {
                id: section.id,
                data: &section.data,
            });
        }

        // Emit any deferred sections
        for section in deferred_sections {
            module.section(&wasm_encoder::RawSection {
                id: section.id,
                data: &section.data,
            });
        }

        // Custom sections (0) and any remaining
        for section in section_iter {
            module.section(&wasm_encoder::RawSection {
                id: section.id,
                data: &section.data,
            });
        }

        Ok(module.finish())
    }
}

/// Translate a wasmparser ValType to a wasm_encoder ValType
fn translate_val_type(ty: wasmparser::ValType) -> Result<wasm_encoder::ValType> {
    match ty {
        wasmparser::ValType::I32 => Ok(wasm_encoder::ValType::I32),
        wasmparser::ValType::I64 => Ok(wasm_encoder::ValType::I64),
        wasmparser::ValType::F32 => Ok(wasm_encoder::ValType::F32),
        wasmparser::ValType::F64 => Ok(wasm_encoder::ValType::F64),
        wasmparser::ValType::V128 => Ok(wasm_encoder::ValType::V128),
        wasmparser::ValType::Ref(r) => translate_ref_type(r),
    }
}

fn translate_ref_type(r: wasmparser::RefType) -> Result<wasm_encoder::ValType> {
    if r.is_func_ref() {
        Ok(wasm_encoder::ValType::Ref(wasm_encoder::RefType::FUNCREF))
    } else if r.is_extern_ref() {
        Ok(wasm_encoder::ValType::Ref(wasm_encoder::RefType::EXTERNREF))
    } else {
        Err(anyhow!("Unsupported reference type"))
    }
}

/// Translate a wasmparser Import into our ParsedImport struct
fn translate_import(import: &wasmparser::Import) -> Result<ParsedImport> {
    let entity = match import.ty {
        wasmparser::TypeRef::Func(idx) => wasm_encoder::EntityType::Function(idx),
        wasmparser::TypeRef::Table(t) => {
            let ref_type = translate_parser_ref_type(t.element_type)?;
            wasm_encoder::EntityType::Table(wasm_encoder::TableType {
                element_type: ref_type,
                minimum: t.initial,
                maximum: t.maximum,
                table64: t.table64,
                shared: false,
            })
        }
        wasmparser::TypeRef::Memory(m) => {
            wasm_encoder::EntityType::Memory(wasm_encoder::MemoryType {
                minimum: m.initial,
                maximum: m.maximum,
                memory64: m.memory64,
                shared: m.shared,
                page_size_log2: m.page_size_log2,
            })
        }
        wasmparser::TypeRef::Global(g) => {
            let val_type = translate_val_type(g.content_type)?;
            wasm_encoder::EntityType::Global(wasm_encoder::GlobalType {
                val_type,
                mutable: g.mutable,
                shared: g.shared,
            })
        }
        wasmparser::TypeRef::Tag(t) => wasm_encoder::EntityType::Tag(wasm_encoder::TagType {
            kind: wasm_encoder::TagKind::Exception,
            func_type_idx: t.func_type_idx,
        }),
        wasmparser::TypeRef::FuncExact(idx) => wasm_encoder::EntityType::FunctionExact(idx),
    };
    Ok(ParsedImport {
        module: import.module.to_string(),
        name: import.name.to_string(),
        ty: entity,
    })
}

/// Translate a wasmparser RefType to a wasm_encoder RefType
fn translate_parser_ref_type(r: wasmparser::RefType) -> Result<wasm_encoder::RefType> {
    if r.is_func_ref() {
        Ok(wasm_encoder::RefType::FUNCREF)
    } else if r.is_extern_ref() {
        Ok(wasm_encoder::RefType::EXTERNREF)
    } else {
        Err(anyhow!("Unsupported reference type for table element"))
    }
}

/// Translate a wasmparser ExternalKind to wasm_encoder ExportKind
fn translate_export_kind(kind: wasmparser::ExternalKind) -> Result<wasm_encoder::ExportKind> {
    match kind {
        wasmparser::ExternalKind::Func => Ok(wasm_encoder::ExportKind::Func),
        wasmparser::ExternalKind::Table => Ok(wasm_encoder::ExportKind::Table),
        wasmparser::ExternalKind::Memory => Ok(wasm_encoder::ExportKind::Memory),
        wasmparser::ExternalKind::Global => Ok(wasm_encoder::ExportKind::Global),
        wasmparser::ExternalKind::Tag => Ok(wasm_encoder::ExportKind::Tag),
        wasmparser::ExternalKind::FuncExact => Ok(wasm_encoder::ExportKind::Func),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_translate_val_types() {
        assert!(translate_val_type(wasmparser::ValType::I32).is_ok());
        assert!(translate_val_type(wasmparser::ValType::I64).is_ok());
        assert!(translate_val_type(wasmparser::ValType::F32).is_ok());
        assert!(translate_val_type(wasmparser::ValType::F64).is_ok());
        assert!(translate_val_type(wasmparser::ValType::V128).is_ok());
    }

    #[test]
    fn test_add_function_type() {
        let wasm = wasm_encoder::Module::new();
        let module_bytes = wasm.finish();

        let mut ctx = ModuleContext::new(&module_bytes).unwrap();
        let idx = ctx.add_function_type(
            vec![wasm_encoder::ValType::I32],
            vec![wasm_encoder::ValType::I64],
        );
        assert_eq!(idx, 0);

        let idx2 = ctx.add_function_type(vec![], vec![]);
        assert_eq!(idx2, 1);
    }

    #[test]
    fn test_roundtrip_empty_module() {
        let wasm = wasm_encoder::Module::new();
        let module_bytes = wasm.finish();

        let ctx = ModuleContext::new(&module_bytes).unwrap();
        let output = ctx.encode().unwrap();

        // Both should be valid minimal modules
        assert!(!output.is_empty());
    }

    #[test]
    fn test_add_function_and_export() {
        let wasm = wasm_encoder::Module::new();
        let module_bytes = wasm.finish();

        let mut ctx = ModuleContext::new(&module_bytes).unwrap();

        // Add a void->void function type
        let type_idx = ctx.add_function_type(vec![], vec![]);

        // Add a function with just an `end` instruction
        let func_idx = ctx.add_function(type_idx, vec![], vec![0x0b]);

        // Export it
        ctx.add_function_export("test_func".to_string(), func_idx);

        let output = ctx.encode().unwrap();
        assert!(!output.is_empty());

        // Re-parse to verify structure
        let ctx2 = ModuleContext::new(&output).unwrap();
        assert_eq!(ctx2.function_by_name("test_func"), Some(0));
    }
}
