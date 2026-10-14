//! Lowering from a [`CheckedProgram`] to a Brim IR [`Module`].
//!
//! Lowering is total: every checked program lowers to a module that passes
//! [`brim_ir::validate`]. Any failure here is a compiler bug, not a user error.
//!
//! Translation scheme:
//! - every local variable (including parameters) gets one slot;
//! - expressions become values in the current block;
//! - `&&` / `||` short-circuit through a temporary `bool` slot and three
//!   blocks (evaluate left, maybe evaluate right, join);
//! - string `+`, `==` and `!=` become runtime calls;
//! - statements after a `return` are unreachable and are not lowered.

use brim_ir::{
    Block, BlockId, Const, FuncId, Function, Inst, InstKind, Module, Slot, SlotId, Span,
    Terminator, Type as IrType, ValueId,
};
use brim_runtime::RuntimeFn;
use brim_semantic::Type;
use brim_semantic::checked::{
    BinaryOp, CheckedExpr, CheckedExprKind, CheckedFn, CheckedProgram, CheckedStmt, UnaryOp,
};

/// Lowers a checked program to IR.
pub fn lower(program: &CheckedProgram) -> Module {
    Module {
        functions: program.functions.iter().map(FnLowerer::lower).collect(),
        entry: func_id(program.entry.0),
    }
}

fn func_id(index: usize) -> FuncId {
    FuncId(u32::try_from(index).expect("function count fits in u32"))
}

fn ir_type(ty: Type) -> IrType {
    match ty {
        Type::I64 => IrType::I64,
        Type::Bool => IrType::Bool,
        Type::String => IrType::Str,
        Type::Void => IrType::Void,
        Type::Error => unreachable!("checked programs never contain error types"),
    }
}

struct FnLowerer {
    func: Function,
    current: BlockId,
    /// Whether the current block already has its terminator.
    terminated: bool,
}

impl FnLowerer {
    fn lower(f: &CheckedFn) -> Function {
        let slots = f
            .locals
            .iter()
            .map(|l| Slot {
                name: l.name.clone(),
                ty: ir_type(l.ty),
            })
            .collect();
        let mut lowerer = FnLowerer {
            func: Function {
                name: f.name.clone(),
                param_count: f.param_count,
                ret: ir_type(f.ret),
                slots,
                values: Vec::new(),
                blocks: Vec::new(),
            },
            current: BlockId(0),
            terminated: false,
        };
        lowerer.current = lowerer.new_block();
        lowerer.stmts(&f.body);
        if !lowerer.terminated {
            // Semantic analysis guarantees non-void functions always return.
            let term = if f.ret == Type::Void {
                Terminator::Return(None)
            } else {
                Terminator::Unreachable
            };
            lowerer.terminate(term);
        }
        lowerer.func
    }

    // ---- building blocks ----------------------------------------------

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.func.blocks.len() as u32);
        self.func.blocks.push(Block {
            insts: Vec::new(),
            term: Terminator::Unreachable,
        });
        id
    }

    fn switch_to(&mut self, block: BlockId) {
        self.current = block;
        self.terminated = false;
    }

    fn terminate(&mut self, term: Terminator) {
        debug_assert!(!self.terminated);
        self.func.blocks[self.current.index()].term = term;
        self.terminated = true;
    }

    fn new_slot(&mut self, name: &str, ty: IrType) -> SlotId {
        let id = SlotId(self.func.slots.len() as u32);
        self.func.slots.push(Slot {
            name: name.to_string(),
            ty,
        });
        id
    }

    /// Appends an instruction. `ty` is the type of its result, if any.
    fn emit(&mut self, kind: InstKind, ty: Option<IrType>, span: Span) -> Option<ValueId> {
        let result = ty.filter(|t| *t != IrType::Void).map(|t| {
            let id = ValueId(self.func.values.len() as u32);
            self.func.values.push(t);
            id
        });
        self.func.blocks[self.current.index()].insts.push(Inst {
            result,
            kind,
            span: Some(span),
        });
        result
    }

    fn emit_value(&mut self, kind: InstKind, ty: IrType, span: Span) -> ValueId {
        self.emit(kind, Some(ty), span)
            .expect("non-void instructions produce a value")
    }

    // ---- statements ---------------------------------------------------

    fn stmts(&mut self, stmts: &[CheckedStmt]) {
        for stmt in stmts {
            if self.terminated {
                // Unreachable code (already warned about) is not lowered.
                return;
            }
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &CheckedStmt) {
        match stmt {
            CheckedStmt::Let { local, init } => {
                let value = self.value(init);
                let slot = SlotId(local.0 as u32);
                self.emit(InstKind::Store(slot, value), None, init.span);
            }
            CheckedStmt::Expr(expr) => {
                self.expr(expr);
            }
            CheckedStmt::Return { value, .. } => {
                let value = value.as_ref().map(|v| self.value(v));
                self.terminate(Terminator::Return(value));
            }
            CheckedStmt::Block(stmts) => self.stmts(stmts),
        }
    }

    // ---- expressions --------------------------------------------------

    /// Lowers an expression that must produce a value.
    fn value(&mut self, expr: &CheckedExpr) -> ValueId {
        self.expr(expr)
            .expect("semantic analysis rejects void values")
    }

    /// Lowers an expression; `None` for calls to `void` functions.
    fn expr(&mut self, expr: &CheckedExpr) -> Option<ValueId> {
        let span = expr.span;
        let ty = ir_type(expr.ty);
        let value = match &expr.kind {
            CheckedExprKind::Int(v) => self.emit_value(InstKind::Const(Const::I64(*v)), ty, span),
            CheckedExprKind::Bool(v) => self.emit_value(InstKind::Const(Const::Bool(*v)), ty, span),
            CheckedExprKind::Str(v) => {
                self.emit_value(InstKind::Const(Const::Str(v.clone())), ty, span)
            }
            CheckedExprKind::Local(local) => {
                self.emit_value(InstKind::Load(SlotId(local.0 as u32)), ty, span)
            }
            CheckedExprKind::Unary { op, operand } => {
                let v = self.value(operand);
                let op = match op {
                    UnaryOp::IntNeg => brim_ir::UnaryOp::INeg,
                    UnaryOp::BoolNot => brim_ir::UnaryOp::BNot,
                };
                self.emit_value(InstKind::Unary(op, v), ty, span)
            }
            CheckedExprKind::Binary { op, lhs, rhs } => {
                let a = self.value(lhs);
                let b = self.value(rhs);
                self.binary(*op, a, b, span)
            }
            CheckedExprKind::And(lhs, rhs) => self.short_circuit(lhs, rhs, true, span),
            CheckedExprKind::Or(lhs, rhs) => self.short_circuit(lhs, rhs, false, span),
            CheckedExprKind::Call { func, args } => {
                let args = args.iter().map(|a| self.value(a)).collect();
                return self.emit(InstKind::Call(func_id(func.0), args), Some(ty), span);
            }
            CheckedExprKind::Builtin { runtime, args } => {
                let args = args.iter().map(|a| self.value(a)).collect();
                return self.emit(InstKind::CallRuntime(*runtime, args), Some(ty), span);
            }
        };
        Some(value)
    }

    fn binary(&mut self, op: BinaryOp, a: ValueId, b: ValueId, span: Span) -> ValueId {
        use brim_ir::BinaryOp as Ir;
        let ir_op = match op {
            BinaryOp::IntAdd => Ir::IAdd,
            BinaryOp::IntSub => Ir::ISub,
            BinaryOp::IntMul => Ir::IMul,
            BinaryOp::IntDiv => Ir::IDiv,
            BinaryOp::IntRem => Ir::IRem,
            BinaryOp::IntEq => Ir::IEq,
            BinaryOp::IntNe => Ir::INe,
            BinaryOp::IntLt => Ir::ILt,
            BinaryOp::IntLe => Ir::ILe,
            BinaryOp::IntGt => Ir::IGt,
            BinaryOp::IntGe => Ir::IGe,
            BinaryOp::BoolEq => Ir::BEq,
            BinaryOp::BoolNe => Ir::BNe,
            BinaryOp::StrConcat => {
                let kind = InstKind::CallRuntime(RuntimeFn::StrConcat, vec![a, b]);
                return self.emit_value(kind, IrType::Str, span);
            }
            BinaryOp::StrEq | BinaryOp::StrNe => {
                let kind = InstKind::CallRuntime(RuntimeFn::StrEq, vec![a, b]);
                let eq = self.emit_value(kind, IrType::Bool, span);
                if op == BinaryOp::StrEq {
                    return eq;
                }
                return self.emit_value(
                    InstKind::Unary(brim_ir::UnaryOp::BNot, eq),
                    IrType::Bool,
                    span,
                );
            }
        };
        let ty = ir_op.signature().1;
        self.emit_value(InstKind::Binary(ir_op, a, b), ty, span)
    }

    /// `lhs && rhs` (`is_and`) or `lhs || rhs`: `rhs` is only evaluated when
    /// `lhs` does not already decide the result.
    fn short_circuit(
        &mut self,
        lhs: &CheckedExpr,
        rhs: &CheckedExpr,
        is_and: bool,
        span: Span,
    ) -> ValueId {
        let result = self.new_slot(if is_and { "and" } else { "or" }, IrType::Bool);
        let l = self.value(lhs);
        self.emit(InstKind::Store(result, l), None, lhs.span);
        let eval_rhs = self.new_block();
        let join = self.new_block();
        let (then_block, else_block) = if is_and {
            (eval_rhs, join)
        } else {
            (join, eval_rhs)
        };
        self.terminate(Terminator::Branch {
            cond: l,
            then_block,
            else_block,
        });

        self.switch_to(eval_rhs);
        let r = self.value(rhs);
        self.emit(InstKind::Store(result, r), None, rhs.span);
        self.terminate(Terminator::Jump(join));

        self.switch_to(join);
        self.emit_value(InstKind::Load(result), IrType::Bool, span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lower_src(src: &str) -> Module {
        let (ast, diags) = brim_parser::parse(src);
        assert!(diags.is_empty(), "{diags:#?}");
        let (checked, diags) = brim_semantic::check(&ast);
        let checked = checked.unwrap_or_else(|| panic!("{diags:#?}"));
        let module = lower(&checked);
        if let Err(errors) = brim_ir::validate(&module) {
            panic!("invalid IR:\n{module}\n{errors:#?}");
        }
        module
    }

    #[test]
    fn lowers_functions_and_calls() {
        let module = lower_src(
            "fn add(a: i64, b: i64) -> i64 {\n    return a + b\n}\nfn main() {\n    let result: i64 = add(10, 32)\n    print(result)\n}\n",
        );
        assert_eq!(
            module.to_string(),
            "; entry: main

fn add(s0 a: i64, s1 b: i64) -> i64 {
bb0:
  v0 = load s0
  v1 = load s1
  v2 = iadd v0, v1
  ret v2
}

fn main() {
  slot s0 result: i64
bb0:
  v0 = const i64 10
  v1 = const i64 32
  v2 = call add(v0, v1)
  store s0, v2
  v3 = load s0
  call @brim_rt_print_i64(v3)
  ret
}
"
        );
    }

    #[test]
    fn lowers_short_circuit_operators() {
        let module = lower_src("fn main() {\n    print(true && false || true)\n}");
        let main = &module.functions[0];
        // `&&` and `||` each add a right-hand-side block and a join block.
        assert_eq!(main.blocks.len(), 5);
        assert_eq!(main.slots.len(), 2);
    }

    #[test]
    fn lowers_string_operations_to_runtime_calls() {
        let module = lower_src(
            "fn main() {\n    let s: string = \"a\" + \"b\"\n    print(s == \"ab\")\n    print(s != \"ab\")\n}",
        );
        let text = module.to_string();
        assert!(text.contains("call @brim_rt_str_concat(v0, v1)"));
        assert_eq!(text.matches("call @brim_rt_str_eq").count(), 2);
        assert!(text.contains("bnot"));
    }

    #[test]
    fn skips_unreachable_code() {
        let module = lower_src("fn f() -> i64 {\n    return 1\n    print(2)\n}\nfn main() {}");
        assert!(!module.to_string().contains("print"));
    }

    #[test]
    fn nested_blocks_and_shadowing_use_distinct_slots() {
        let module = lower_src(
            "fn main() {\n    let x: i64 = 1\n    {\n        let x: string = \"s\"\n        print(x)\n    }\n    print(x)\n}",
        );
        let main = &module.functions[0];
        assert_eq!(main.slots.len(), 2);
        assert_eq!(main.slots[0].ty, IrType::I64);
        assert_eq!(main.slots[1].ty, IrType::Str);
    }
}
