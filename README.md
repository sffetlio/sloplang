# sloplang

A low-level, compiled, concurrent programming language designed to be written by AI
models. Human readability is not a goal; correctness on the first try, token
efficiency and speed are.

```lisp
(fn count_primes ((n i64)) i64
  (doc "parallel prime count below n")
  (eff mem conc)
  (let flags (alloc u8 n))
  (pfor i 0 n (if (is_prime i) (put flags i 1u8)))
  (var total 0)
  (for i 0 n (set total (+ total (as i64 (get flags i)))))
  (free flags)
  total)
```

## Why it suits a model

- **One way to write everything.** S-expressions, no precedence, no comments, one
  enforced layout (`slopc fmt`), no shadowing. Less ambiguity means fewer sampling errors.
- **Signatures tell the whole story.** Types, effects (`io mem conc`), ownership and
  contracts are all in the signature, and `slopc sig` prints just that, so a model can
  call into code it has never read.
- **Mistakes are compile errors, not heisenbugs.** Linear buffers and tasks, mandatory
  joins (structured concurrency) and a checked-disjoint `pfor` rule out data races,
  leaked tasks and double frees statically.
- **Errors are JSON with a `fix`.** Every diagnostic has a stable code, position and,
  where possible, the exact replacement text. Runtime traps are JSON too.
- **Built-in tests and property tests** with deterministic seeds, contracts with `pre`/`post`.
- **Fast.** Compiles to C; with `--no-checks`, recursive `fib(40)` runs at the same speed
  as hand-written C (about 0.2 s here), and `pfor` scales across cores.

## Quick start

```sh
cargo build --release
./target/release/slopc run examples/workers.slop
./target/release/slopc test examples/fib.slop
./target/release/slopc check tests/errors/E0305_pfor_race.slop
```

Requires Rust and a C compiler (gcc or clang) with pthreads.

## Docs

- [docs/spec.md](docs/spec.md): the language specification, with what v0 implements and what is planned.
- [docs/primer.md](docs/primer.md): a dense, roughly 1k-token primer to paste into a model's context.
- `examples/`: runnable programs with recorded output; `examples/traps.slop` fails on purpose to show trap and counterexample reports.
- `tests/errors/`: one file per diagnostic, named by the code it must produce.

## Layout

| Path | Contents |
|---|---|
| `src/sexp.rs` | reader and canonical formatter |
| `src/parse.rs` | S-expressions to AST |
| `src/check.rs` | types, effects, ownership, pfor disjointness |
| `src/codegen.rs` | C backend, test harness |
| `src/runtime.h` | embedded runtime: checked arithmetic, buffers, tasks, channels, pfor |
| `src/main.rs` | `slopc` CLI |
