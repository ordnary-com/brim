# Security Policy

## Supported versions

Brim is pre-release software (0.x). Only the latest commit on the default branch receives fixes.

## Reporting a vulnerability

Please **do not** open a public issue for security problems. Instead, report them privately to Ordnary through GitHub's private vulnerability reporting ("Report a vulnerability" on the repository's Security tab). Include:

- a description of the issue and its impact;
- a minimal Brim program, manifest or command line that reproduces it;
- the output of `brim --version` and your platform.

We aim to acknowledge reports within five working days.

## Scope

Examples of issues we consider security-relevant:

- the compiler or interpreter crashing, hanging or exhausting memory on crafted input (the toolchain is meant to never panic on any input);
- reading or writing files outside what a command is asked to process;
- once native code generation exists: miscompilation that breaks the language's guarantees, such as unchecked integer overflow.

Brim 0.1 makes **no memory-safety guarantees for future native code**. See the "Memory management" section of `docs/architecture.md` for what is and is not guaranteed today.
