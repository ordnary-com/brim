use super::*;

fn inst(result: Option<u32>, kind: InstKind) -> Inst {
    Inst {
        result: result.map(ValueId),
        kind,
        span: None,
    }
}

/// `fn add(a: i64, b: i64) -> i64 { return a + b }`
fn add_fn() -> Function {
    Function {
        name: "add".into(),
        param_count: 2,
        ret: Type::I64,
        slots: vec![
            Slot {
                name: "a".into(),
                ty: Type::I64,
            },
            Slot {
                name: "b".into(),
                ty: Type::I64,
            },
        ],
        values: vec![Type::I64; 3],
        blocks: vec![Block {
            insts: vec![
                inst(Some(0), InstKind::Load(SlotId(0))),
                inst(Some(1), InstKind::Load(SlotId(1))),
                inst(
                    Some(2),
                    InstKind::Binary(BinaryOp::IAdd, ValueId(0), ValueId(1)),
                ),
            ],
            term: Terminator::Return(Some(ValueId(2))),
        }],
    }
}

/// `fn main() { print(add(10, 32)) }`
fn main_fn() -> Function {
    Function {
        name: "main".into(),
        param_count: 0,
        ret: Type::Void,
        slots: vec![],
        values: vec![Type::I64; 3],
        blocks: vec![Block {
            insts: vec![
                inst(Some(0), InstKind::Const(Const::I64(10))),
                inst(Some(1), InstKind::Const(Const::I64(32))),
                inst(
                    Some(2),
                    InstKind::Call(FuncId(0), vec![ValueId(0), ValueId(1)]),
                ),
                inst(
                    None,
                    InstKind::CallRuntime(RuntimeFn::PrintI64, vec![ValueId(2)]),
                ),
            ],
            term: Terminator::Return(None),
        }],
    }
}

fn module() -> Module {
    Module {
        structs: vec![],
        lists: vec![],
        maps: vec![],
        functions: vec![add_fn(), main_fn()],
        entry: FuncId(1),
    }
}

fn errors(module: &Module) -> Vec<String> {
    validate(module)
        .err()
        .unwrap_or_default()
        .into_iter()
        .map(|e| e.to_string())
        .collect()
}

#[test]
fn valid_module_passes() {
    assert_eq!(validate(&module()), Ok(()));
}

#[test]
fn textual_form() {
    assert_eq!(
        module().to_string(),
        "; entry: main

fn add(s0 a: i64, s1 b: i64) -> i64 {
bb0:
  v0 = load s0
  v1 = load s1
  v2 = iadd v0, v1
  ret v2
}

fn main() {
bb0:
  v0 = const i64 10
  v1 = const i64 32
  v2 = call add(v0, v1)
  call @brim_rt_print_i64(v2)
  ret
}
"
    );
}

#[test]
fn rejects_type_errors() {
    let mut m = module();
    m.functions[0].blocks[0].insts[2].kind =
        InstKind::Binary(BinaryOp::BEq, ValueId(0), ValueId(1));
    let errs = errors(&m);
    assert!(errs.contains(&"in `add` bb0: v0 has type i64, expected bool".to_string()));
    assert!(errs.contains(&"in `add` bb0: v2 is declared as i64 but assigned bool".to_string()));
}

#[test]
fn rejects_use_before_definition_and_cross_block_values() {
    let mut m = module();
    m.functions[0].blocks[0].insts.swap(0, 2);
    assert!(
        errors(&m).contains(&"in `add` bb0: v0 is used before being defined in this block".into())
    );

    let mut m = module();
    let f = &mut m.functions[0];
    f.blocks[0].term = Terminator::Jump(BlockId(1));
    f.blocks.push(Block {
        insts: vec![],
        term: Terminator::Return(Some(ValueId(2))),
    });
    assert_eq!(
        errors(&m),
        vec!["in `add` bb1: v2 is used before being defined in this block"]
    );
}

#[test]
fn rejects_double_definition() {
    let mut m = module();
    m.functions[0].blocks[0].insts[1].result = Some(ValueId(0));
    assert!(errors(&m).contains(&"in `add` bb0: v0 is defined more than once".into()));
}

#[test]
fn rejects_bad_calls() {
    let mut m = module();
    m.functions[1].blocks[0].insts[2].kind = InstKind::Call(FuncId(0), vec![ValueId(0)]);
    assert_eq!(
        errors(&m),
        vec!["in `main` bb0: `add` expects 2 arguments, got 1"]
    );
    let mut m = module();
    m.functions[1].blocks[0].insts[3].kind =
        InstKind::CallRuntime(RuntimeFn::PrintStr, vec![ValueId(2)]);
    assert_eq!(
        errors(&m),
        vec!["in `main` bb0: v2 has type i64, expected str"]
    );
    let mut m = module();
    m.functions[1].blocks[0].insts[2].kind = InstKind::Call(FuncId(9), vec![]);
    assert!(errors(&m).contains(&"in `main` bb0: call to nonexistent fn9".into()));
}

#[test]
fn rejects_bad_terminators() {
    let mut m = module();
    m.functions[0].blocks[0].term = Terminator::Return(None);
    assert_eq!(
        errors(&m),
        vec!["in `add` bb0: missing return value of type i64"]
    );
    let mut m = module();
    m.functions[1].blocks[0].term = Terminator::Branch {
        cond: ValueId(0),
        then_block: BlockId(0),
        else_block: BlockId(7),
    };
    assert_eq!(
        errors(&m),
        vec![
            "in `main` bb0: v0 has type i64, expected bool",
            "in `main` bb0: branch to nonexistent bb7"
        ]
    );
}

#[test]
fn rejects_bad_entry_and_slots() {
    let mut m = module();
    m.entry = FuncId(0);
    assert_eq!(
        errors(&m),
        vec!["in `add`: the entry function must take no parameters and return void"]
    );
    let mut m = module();
    m.functions[0].slots[1].ty = Type::Void;
    assert!(errors(&m).contains(&"in `add`: slot s1 has type void".into()));
}

/// `struct %0 Pair { a: i64, b: str }` and a `main` that builds, reads and
/// updates one.
fn struct_module(insts: Vec<InstKind>, values: Vec<Type>) -> Module {
    Module {
        lists: vec![],
        maps: vec![],
        structs: vec![StructDef {
            name: "Pair".into(),
            fields: vec![
                Field {
                    name: "a".into(),
                    ty: Type::I64,
                },
                Field {
                    name: "b".into(),
                    ty: Type::Str,
                },
            ],
        }],
        functions: vec![Function {
            name: "main".into(),
            param_count: 0,
            ret: Type::Void,
            slots: vec![],
            blocks: vec![Block {
                insts: insts
                    .into_iter()
                    .enumerate()
                    .map(|(i, kind)| Inst {
                        result: Some(ValueId(i as u32)),
                        kind,
                        span: None,
                    })
                    .collect(),
                term: Terminator::Return(None),
            }],
            values,
        }],
        entry: FuncId(0),
    }
}

#[test]
fn struct_instructions() {
    let pair = Type::Struct(StructId(0));
    let valid = struct_module(
        vec![
            InstKind::Const(Const::I64(1)),
            InstKind::Const(Const::Str("x".into())),
            InstKind::MakeStruct(StructId(0), vec![ValueId(0), ValueId(1)]),
            InstKind::Extract(ValueId(2), 1),
            InstKind::Insert(ValueId(2), 0, ValueId(0)),
        ],
        vec![Type::I64, Type::Str, pair, Type::Str, pair],
    );
    assert_eq!(validate(&valid), Ok(()));
    assert!(valid.to_string().contains(
        "struct %0 Pair { a: i64, b: str }\n\nfn main() {\nbb0:\n  v0 = const i64 1\n  v1 = const str \"x\"\n  v2 = make %0(v0, v1)\n  v3 = extract v2.1\n  v4 = insert v2.0, v0\n"
    ));

    let wrong = struct_module(
        vec![
            InstKind::Const(Const::I64(1)),
            InstKind::MakeStruct(StructId(0), vec![ValueId(0), ValueId(0)]),
            InstKind::Extract(ValueId(1), 5),
            InstKind::Extract(ValueId(0), 0),
        ],
        vec![Type::I64, pair, Type::I64, Type::I64],
    );
    assert_eq!(
        errors(&wrong),
        [
            "in `main` bb0: v0 has type i64, expected str",
            "in `main` bb0: %0 has no field 5",
            "in `main` bb0: v0 has type i64, expected a struct",
        ]
    );
}

#[test]
fn structs_must_not_contain_themselves() {
    let mut module = struct_module(vec![], vec![]);
    module.structs[0].fields[1].ty = Type::Struct(StructId(0));
    assert_eq!(
        errors(&module),
        ["in `struct Pair`: struct contains itself"]
    );
}
