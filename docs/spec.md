# sloplang specification, v0

sloplang is a low-level, compiled, concurrent language whose primary author is a
language model. Human readability is not a design goal. Every decision below is
judged by one question: does it make a model produce correct, fast code with fewer
tokens, fewer retries and less context?

The reference compiler `slopc` (Rust, no dependencies) type-checks a single `.slop`
file and lowers it to C11 (GNU dialect), which the system C compiler turns into a
native binary. Status markers: **[v0]** is implemented in this repository,
**[planned]** is specified but not built yet.

---

## 1. Design principles

| Principle | Why it helps a model | How it shows up |
|---|---|---|
| One canonical form | Generation is sampling; fewer valid spellings means less probability mass on wrong ones | S-expressions, one layout (`slopc fmt`), one naming convention, no operator precedence, no optional syntax |
| Locality | A model sees a window, not the codebase | Every fn signature carries full types, effects and contracts; no type inference across functions; no shadowing |
| Static facts over runtime surprises | Retrying after a runtime failure is expensive | Ownership, effect and data-race checks at compile time |
| Loud, structured failure | A model fixes errors from text it can parse | JSON diagnostics with stable codes and a `fix` field; JSON traps with source positions |
| No implicit behaviour | Implicit rules are where models hallucinate | No implicit numeric conversion, no implicit copies of resources, no exceptions, no GC, no default effects |
| Tokenizer-friendly | BPE vocabularies favour short ASCII words | ASCII only, keywords are common short English words (`let`, `get`, `put`, `go`, `wait`), no sigils beyond arithmetic |
| Low level | Speed is a requirement | Fixed-width integers, flat buffers, value structs, no hidden allocation, threads and atomics are first class |

## 2. Lexical structure [v0]

- A file is a sequence of top-level forms. A form is an atom or `( form* )`.
- Atoms:
  - integers: `42`, `-7`, `0xff`, with an optional type suffix `i8 i16 i32 i64 u8 u16 u32 u64` (`255u8`).
  - floats: `1.5`, `-2.0e-3`, optional suffix `f32 f64`; `1f32` is a float.
  - strings: `"..."` with escapes `\n \t \\ \"`. Strings exist only for `print`.
  - symbols: everything else.
- Whitespace separates tokens. There are **no comments**: `;` is error `E0004`.
  Documentation is data: `(doc "...")` inside `fn` or `struct`, which survives
  formatting and appears in `slopc sig`.
- Names: values and functions match `[a-z][a-z0-9_]*`; structs match `[A-Z][A-Za-z0-9]*`.
  Reserved words (every keyword, primitive and type name) are rejected with `E0104`.

## 3. Top-level forms [v0]

```
(fn NAME ((PARAM TYPE)...) RET
  [(eff io mem conc)] [(doc "...")] [(pre EXPR)]... [(post EXPR)]...
  BODY...)
(struct Name (field TYPE)...)
(test NAME BODY...)
(prop NAME ((VAR INT-TYPE LO HI)...) BODY... BOOL-EXPR)
```

- Order of top-level forms does not matter; functions may be mutually recursive.
- `main` is `(fn main () i64 ...)` (exit code) or `(fn main () unit ...)`.
- A function's value is its last body expression. `unit` functions discard it.

## 4. Types [v0]

| Type | Meaning | C lowering |
|---|---|---|
| `i8 i16 i32 i64 u8 u16 u32 u64` | fixed-width integers | `intN_t` / `uintN_t` |
| `f32 f64` | IEEE floats | `float` / `double` |
| `bool` | `true` / `false` | `bool` |
| `unit` | no information; value of statements | `uint8_t` |
| `str` | static string literal, printable only | `const char *` |
| `Name` | struct, a plain value (copied) | C struct |
| `(buf T)` | owned, mutable, bounds-checked heap array | `{ptr, len}` |
| `(ro T)` | frozen, shareable, read-only array | `{ptr, len}` |
| `(chan T)` | bounded MPMC channel handle (shareable) | pointer |
| `(task T)` | running task returning `T` | pointer |
| `(group T)` | set of tasks returning `T` | pointer |

Rules:
- No implicit conversions. `(as T x)` converts numerics (and `bool` to a number).
  Integer narrowing and float-to-integer conversion are range checked (trap `T0011`).
- Literals take the type the context expects (`(+ x 1)` with `x: u8` makes `1` a `u8`);
  with no expectation an integer literal is `i64` and a float literal is `f64`.
- Element types of `buf`, `ro`, `group` are numbers, `bool` or structs. A `chan` may
  also carry `(buf T)`, which moves ownership through the channel.
- Struct fields are numbers, `bool` or structs. Recursive structs are rejected.

## 5. Memory and ownership model [v0]

There is no garbage collector and no hidden allocation. Memory is explicit, and the
checker enforces a small ownership discipline that makes data races and double frees
impossible in safe code without lifetime annotations.

1. **Linear values.** `buf`, `task` and `group` are moved, never copied. Passing a
   linear variable to something that takes ownership marks it moved; any later use is
   `E0301`. Ownership is taken by `free`, `freeze`, `join`, `wait`, `send`, `spawn`/`go`
   arguments, `let`/`set` from a variable, returning it, and calls described in (3).
2. **Tasks must be joined.** `task` and `group` values must be consumed exactly once, in
   the scope that owns them (`E0303`, `E0306`, `E0307`). This is structured concurrency:
   no task outlives its creator unless it is explicitly returned.
3. **Ownership follows the return type.** A function whose return type is
   `buf`, `task` or `group` owns its buffer parameters (the caller's variables are
   moved). Every other function borrows them: it may read and write elements but may
   not free, freeze, send or spawn with them (`E0308`). No annotations needed: the
   signature says it.
4. **Loops.** A linear value declared outside a loop cannot be moved inside it
   (`E0302`): there is no second iteration to give it back.
5. **Freezing.** `(freeze b)` turns `(buf T)` into `(ro T)`, which is copyable and
   shareable across tasks because nothing can write it.
6. **Traps, not undefined behaviour.** Integer overflow (`T0001`), division by zero
   (`T0002`), out-of-bounds access (`T0003`), bad shifts (`T0009`) and bad casts
   (`T0011`) trap with a JSON report. Wrapping arithmetic is explicit: `+% -% *%`.
   `--no-checks` removes these checks for release builds; the program's meaning is
   unchanged if it never trapped.

Known holes in v0: freed buffers are tracked only through the variable that was
freed; ownership of buffers returned from borrowed calls is not tracked beyond the
rule in (3); channels and frozen buffers are never reclaimed.

## 6. Concurrency model [v0]

| Construct | Meaning |
|---|---|
| `(spawn (f args...))` | start `f` on a new task, returns `(task R)`; linear args move into it |
| `(join t)` | wait for `t`, returns its result |
| `(group T)`, `(go g (f args...))`, `(wait g)` | task group: start many tasks, `wait` joins all and returns `(buf T)` of results in start order |
| `(chan T cap)`, `(send c v)`, `(recv c)`, `(close c)`, `(each x c body...)` | bounded channels; `each` drains until closed; `recv` on a closed empty channel traps `T0007` |
| `(pfor i lo hi body...)` | data-parallel loop over `[lo, hi)` split across worker threads |
| `(xadd b i v)`, `(cas b i old new)` | sequentially consistent atomics on integer buffers |

Data-race freedom comes from three static rules:
- linear values move into tasks, so two tasks never hold the same `buf`;
- only `ro`, channels and scalars are shared;
- `pfor` bodies are checked for disjointness: an outer buffer that is written in the
  body may only be accessed at the loop index (`(put b i ...)`, `(get b i)`); atomics
  on a buffer may not mix with plain access to it; outer variables cannot be assigned
  (`E0304`, `E0305`). Calls that receive an outer buffer inside `pfor` are allowed only
  if a whole-program analysis proves the callee never writes it.

Runtime: v0 maps each task to an OS thread and splits `pfor` into contiguous chunks,
one per core (`SLOP_THREADS` overrides). **[planned]** M:N work-stealing scheduler,
`select` over channels, deterministic replay scheduler for tests, reductions
(`(pfor i lo hi (sum acc) ...)`).

## 7. Effects [v0]

A function declares what it does beyond computing: `io` (print, clock), `mem`
(alloc, free), `conc` (tasks, channels, pfor). A call is allowed only if the caller
declares every effect of the callee (`E0251`, with the corrected `(eff ...)` as the
`fix`). Functions without `(eff ...)` are pure, so a model can trust a signature
without reading the body. Tests and props may use every effect.

## 8. Contracts [v0]

`(pre e)` is checked on entry, `(post e)` on exit, where `ret` names the result.
Contracts must be pure (`E0252`). Failures trap with `T0004`/`T0005` and the contract's
source text. They are part of `slopc sig`, so they document the interface for the next
model that calls the function. `--no-checks` removes them. **[planned]** discharge
contracts statically with an SMT solver and report counterexamples as JSON.

## 9. Built-in testing [v0]

- `(test name body...)`: runs the body; any trap fails the test with the trap report.
- `(prop name ((x i64 lo hi)...) body... bool)`: property test. Runs 256 cases: all-low,
  all-high, then deterministic pseudo-random values seeded by the prop name, so a
  failure reproduces exactly. A failure reports the counterexample as JSON.
- `slopc test FILE` prints one JSON object per test and a summary; exit code 1 on failure.
- **[planned]** shrinking, coverage-guided fuzzing of `pre` contracts, golden-output tests.

## 10. Tooling and SDLC features

| Command | Purpose | Status |
|---|---|---|
| `slopc check` | all diagnostics as JSON lines with `code`, `line`, `col`, `msg`, `fix` | [v0] |
| `slopc explain CODE` | JSON description of any diagnostic or trap code | [v0] |
| `slopc fmt [--check\|--write]` | canonical layout; `--check` for CI | [v0] |
| `slopc sig` | the file's interface: every fn and struct with types, effects, docs, contracts, no bodies. A model loads this instead of the source to call into a module | [v0] |
| `slopc hash` | per-definition content hash, independent of layout. Enables caching, "what changed" diffs and, later, content-addressed imports | [v0] |
| `slopc build --no-checks` | release build | [v0] |
| deterministic builds | generated C is a pure function of the source; no timestamps, stable ordering | [v0] |
| modules | `(use "path.slop")` with imports pinned by content hash | [planned] |
| generics | monomorphized type parameters on fn and struct | [planned] |
| SIMD | `(vec f32 8)` types and lane-wise ops | [planned] |
| JSON AST I/O | `slopc ast` / `slopc from-ast`, so models can emit trees directly | [planned] |
| native backend | direct LLVM or Cranelift output instead of C | [planned] |

## 11. Diagnostics and traps

Each diagnostic is one line of JSON:

```json
{"sev":"error","code":"E0251","file":"a.slop","line":1,"col":17,"msg":"fn main performs effect 'io' but does not declare it","fix":"(eff io)"}
```

Traps go to stderr with exit code 101:

```json
{"kind":"trap","code":"T0003","msg":"index out of bounds","file":"a.slop","line":15,"col":41}
```

The full code table lives in `src/diag.rs` and is printed by `slopc explain`.

## 12. Expression reference [v0]

| Form | Type |
|---|---|
| `(let x [T] e)`, `(var x [T] e)` | binds in the rest of the block; `var` is mutable |
| `(set place e)` | place is `x` or `(. place field)` |
| `(if c a b)`, `(if c a)` | branches must agree; one-armed `if` is `unit` |
| `(do e...)` | block, value of last expression |
| `(while c body...)`, `(for i lo hi body...)` | `unit`; `for` runs `lo <= i < hi` |
| `(break)`, `(continue)`, `(ret [e])` | control flow |
| `+ - * / %` | checked arithmetic, same-type operands; `(- x)` negates |
| `+% -% *%` | wrapping arithmetic |
| `& \| ^ << >>` | bitwise, integers only |
| `== != < <= > >=` | comparison, returns `bool` |
| `and or not` | short-circuit boolean logic |
| `(as T e)` | numeric conversion |
| `(mk S fields...)`, `(. e field)` | struct construction and access |
| `(alloc T n)`, `(get b i)`, `(put b i v)`, `(len b)`, `(free b)`, `(freeze b)` | buffers |
| `(print e...)`, `(clock)` | io: space-separated values plus newline; monotonic nanoseconds |
| `(assert c)` | traps `T0006` with the source text |

Evaluation order is strictly left to right, including function arguments.
