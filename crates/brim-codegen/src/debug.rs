//! DWARF debug information for debug builds: a line table that maps the
//! code to Brim source lines, a subprogram per function with its Brim
//! name, and the variables of scalar types, so lldb and gdb can stop at a
//! line (`b main.brim:12`), step, show the call stack and print numbers
//! and booleans.

use std::collections::HashMap;

use brim_diagnostics::{SourceMap, Sources, Span};
use brim_ir::Type;
use inkwell::context::Context;
use inkwell::debug_info::{
    AsDIScope, DIFile, DIFlags, DIFlagsConstants, DILocation, DISubprogram, DIType,
    DWARFEmissionKind, DWARFSourceLanguage, DebugInfoBuilder,
};
use inkwell::module::{FlagBehavior, Module};
use inkwell::values::{FunctionValue, PointerValue};

/// `DW_ATE_*` encodings.
const BOOLEAN: u32 = 0x02;
const FLOAT: u32 = 0x04;
const SIGNED: u32 = 0x05;

pub(crate) struct DebugInfo<'ctx> {
    builder: DebugInfoBuilder<'ctx>,
    /// One per file of the source map, in order.
    files: Vec<DIFile<'ctx>>,
    types: HashMap<&'static str, DIType<'ctx>>,
}

impl<'ctx> DebugInfo<'ctx> {
    pub(crate) fn new(context: &'ctx Context, module: &Module<'ctx>, source: &SourceMap) -> Self {
        let dir = std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        let main = source.main_file().name().to_string();
        let (builder, _unit) = module.create_debug_info_builder(
            true,
            DWARFSourceLanguage::C,
            &main,
            &dir,
            concat!("brim ", env!("CARGO_PKG_VERSION")),
            false,
            "",
            0,
            "",
            DWARFEmissionKind::Full,
            0,
            false,
            false,
            "",
            "",
        );
        let files = source
            .files()
            .iter()
            .map(|f| builder.create_file(f.name(), &dir))
            .collect();
        let i32_type = context.i32_type();
        module.add_basic_value_flag(
            "Debug Info Version",
            FlagBehavior::Warning,
            i32_type.const_int(3, false),
        );
        module.add_basic_value_flag(
            "Dwarf Version",
            FlagBehavior::Warning,
            i32_type.const_int(4, false),
        );
        let mut types = HashMap::new();
        for (name, bits, encoding) in [
            ("i64", 64, SIGNED),
            ("f64", 64, FLOAT),
            ("bool", 8, BOOLEAN),
        ] {
            if let Ok(ty) = builder.create_basic_type(name, bits, encoding, DIFlags::PUBLIC) {
                types.insert(name, ty.as_type());
            }
        }
        DebugInfo {
            builder,
            files,
            types,
        }
    }

    /// The file and line of a global source offset.
    fn place(&self, source: &SourceMap, span: Span) -> (DIFile<'ctx>, u32, u32) {
        let (file, base, index) = source.locate(span.start);
        let at = file.location(span.start - base);
        (self.files[index], at.line as u32, at.column as u32)
    }

    /// Attaches a subprogram to `function`, at the first span of its code.
    pub(crate) fn function(
        &self,
        source: &SourceMap,
        function: FunctionValue<'ctx>,
        name: &str,
        first: Option<Span>,
    ) -> (DISubprogram<'ctx>, DIFile<'ctx>) {
        let (file, line, _) = match first {
            Some(span) => self.place(source, span),
            None => (self.files[0], 0, 0),
        };
        let ty = self
            .builder
            .create_subroutine_type(file, None, &[], DIFlags::PUBLIC);
        let linkage = function.get_name().to_string_lossy().into_owned();
        let sp = self.builder.create_function(
            file.as_debug_info_scope(),
            name,
            Some(&linkage),
            file,
            line,
            ty,
            true,
            true,
            line,
            DIFlags::PUBLIC,
            false,
        );
        function.set_subprogram(sp);
        (sp, file)
    }

    /// The location of `span` (or line 0 without one) in `scope`.
    pub(crate) fn location(
        &self,
        context: &'ctx Context,
        source: &SourceMap,
        scope: DISubprogram<'ctx>,
        span: Option<Span>,
    ) -> DILocation<'ctx> {
        let (line, column) = match span {
            Some(span) => {
                let (_, line, column) = self.place(source, span);
                (line, column)
            }
            None => (0, 0),
        };
        self.builder
            .create_debug_location(context, line, column, scope.as_debug_info_scope(), None)
    }

    /// Declares a variable stored at `slot`, for types a debugger can show
    /// as they are.
    pub(crate) fn variable(
        &self,
        (scope, file): (DISubprogram<'ctx>, DIFile<'ctx>),
        name: &str,
        ty: Type,
        slot: PointerValue<'ctx>,
        location: DILocation<'ctx>,
        block: inkwell::basic_block::BasicBlock<'ctx>,
    ) {
        let key = match ty {
            Type::I64 => "i64",
            Type::F64 => "f64",
            Type::Bool => "bool",
            _ => return,
        };
        let Some(di_type) = self.types.get(key) else {
            return;
        };
        let var = self.builder.create_auto_variable(
            scope.as_debug_info_scope(),
            name,
            file,
            location.get_line(),
            *di_type,
            true,
            DIFlags::ZERO,
            0,
        );
        let expr = self.builder.create_expression(Vec::new());
        brim_llvm_extra::declare_at_end(&self.builder, slot, var, expr, location, block);
    }

    pub(crate) fn finish(&self) {
        self.builder.finalize();
    }
}
