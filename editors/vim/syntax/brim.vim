" Vim syntax file for Brim.
if exists("b:current_syntax")
  finish
endif

syn keyword brimKeyword if else while for in match return break continue
syn keyword brimStorage fn struct enum impl let mut pub
syn keyword brimInclude import as
syn keyword brimBoolean true false
syn keyword brimSelf self
syn keyword brimReserved async await const loop trait type use
syn keyword brimType i64 f64 bool string void Option Result List Map
syn keyword brimConstant Some None Ok Err
syn keyword brimBuiltin print eprint to_string to_float to_int read_line read_file write_file args env exit assert assert_eq
syn match brimNumber "\<[0-9][0-9_]*\(\.[0-9][0-9_]*\)\=\([eE][+-]\=[0-9]\+\)\=\>"
syn match brimEscape contained "\\[nrt0\"\\]"
syn region brimString start=+"+ end=+"\|$+ contains=brimEscape
syn match brimComment "//.*$"
syn match brimTypeName "\<[A-Z][A-Za-z0-9_]*\>"

hi def link brimKeyword Statement
hi def link brimStorage StorageClass
hi def link brimInclude Include
hi def link brimBoolean Boolean
hi def link brimSelf Identifier
hi def link brimReserved Error
hi def link brimType Type
hi def link brimConstant Constant
hi def link brimBuiltin Function
hi def link brimNumber Number
hi def link brimEscape SpecialChar
hi def link brimString String
hi def link brimComment Comment
hi def link brimTypeName Type

let b:current_syntax = "brim"
