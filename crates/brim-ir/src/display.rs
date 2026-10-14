//! The textual form of Brim IR, used by `brim check --emit ir` and in tests.
//!
//! ```text
//! fn add(s0 a: i64, s1 b: i64) -> i64 {
//! bb0:
//!   v0 = load s0
//!   v1 = load s1
//!   v2 = iadd v0, v1
//!   ret v2
//! }
//! ```

use std::fmt::{self, Display, Formatter};

use crate::{Const, Function, Inst, InstKind, Module, Terminator, ValueId};

fn list(values: &[ValueId]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

impl Display for Module {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "; entry: {}", self.function(self.entry).name)?;
        for (i, def) in self.structs.iter().enumerate() {
            let fields: Vec<String> = def
                .fields
                .iter()
                .map(|field| format!("{}: {}", field.name, field.ty))
                .collect();
            writeln!(f, "struct %{i} {} {{ {} }}", def.name, fields.join(", "))?;
        }
        for function in &self.functions {
            writeln!(f)?;
            function.fmt_in(self, f)?;
        }
        Ok(())
    }
}

impl Function {
    fn fmt_in(&self, module: &Module, f: &mut Formatter<'_>) -> fmt::Result {
        let params: Vec<String> = self
            .slots
            .iter()
            .enumerate()
            .take(self.param_count)
            .map(|(i, s)| format!("s{i} {}: {}", s.name, s.ty))
            .collect();
        write!(f, "fn {}({})", self.name, params.join(", "))?;
        if self.ret != crate::Type::Void {
            write!(f, " -> {}", self.ret)?;
        }
        writeln!(f, " {{")?;
        for (i, slot) in self.slots.iter().enumerate().skip(self.param_count) {
            writeln!(f, "  slot s{i} {}: {}", slot.name, slot.ty)?;
        }
        for (i, block) in self.blocks.iter().enumerate() {
            writeln!(f, "bb{i}:")?;
            for inst in &block.insts {
                writeln!(f, "  {}", InstDisplay { inst, module })?;
            }
            writeln!(f, "  {}", block.term)?;
        }
        writeln!(f, "}}")
    }
}

struct InstDisplay<'a> {
    inst: &'a Inst,
    module: &'a Module,
}

impl Display for InstDisplay<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if let Some(result) = self.inst.result {
            write!(f, "{result} = ")?;
        }
        match &self.inst.kind {
            InstKind::Const(Const::I64(v)) => write!(f, "const i64 {v}"),
            InstKind::Const(Const::Bool(v)) => write!(f, "const bool {v}"),
            InstKind::Const(Const::Str(v)) => write!(f, "const str {v:?}"),
            InstKind::Load(slot) => write!(f, "load {slot}"),
            InstKind::Store(slot, value) => write!(f, "store {slot}, {value}"),
            InstKind::Unary(op, v) => write!(f, "{} {v}", op.mnemonic()),
            InstKind::Binary(op, a, b) => write!(f, "{} {a}, {b}", op.mnemonic()),
            InstKind::Call(func, args) => {
                let name = self
                    .module
                    .functions
                    .get(func.index())
                    .map_or("<invalid>", |f| f.name.as_str());
                write!(f, "call {name}({})", list(args))
            }
            InstKind::CallRuntime(rt, args) => write!(f, "call @{rt}({})", list(args)),
            InstKind::MakeStruct(id, args) => write!(f, "make {id}({})", list(args)),
            InstKind::Extract(v, index) => write!(f, "extract {v}.{index}"),
            InstKind::Insert(v, index, x) => write!(f, "insert {v}.{index}, {x}"),
        }
    }
}

impl Display for Terminator {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Terminator::Return(None) => f.write_str("ret"),
            Terminator::Return(Some(v)) => write!(f, "ret {v}"),
            Terminator::Jump(b) => write!(f, "jump {b}"),
            Terminator::Branch {
                cond,
                then_block,
                else_block,
            } => write!(f, "branch {cond}, {then_block}, {else_block}"),
            Terminator::Unreachable => f.write_str("unreachable"),
        }
    }
}
