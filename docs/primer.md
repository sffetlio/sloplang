sloplang v0 primer. Paste into a model's context. Full spec: docs/spec.md.

SYNTAX: S-exprs only. No comments; use (doc "..") in fn/struct. Names [a-z][a-z0-9_]*, structs [A-Z][A-Za-z0-9]*. No shadowing. No precedence. Left-to-right evaluation.
TOP: (fn f ((a T)...) R [(eff io mem conc)] [(doc "")] [(pre e)] [(post e)] body...)  last expr = result; `ret` = result in post
     (struct S (f T)...)  (test n body...)  (prop n ((x i64 lo hi)...) body... bool)  main: (fn main () i64 ...)
TYPES: i8..i64 u8..u64 f32 f64 bool unit str S (buf T) (ro T) (chan T) (task T) (group T). No implicit conversion: (as T x). Literals adopt expected type; 1u8 2.0f32 0xff.
EXPR: (let x [T] e) (var x [T] e) (set x e) (set (. s f) e) (if c a [b]) (do ...) (while c ...) (for i lo hi ...) (break) (continue) (ret [e])
      + - * / % (checked, trap) +% -% *% (wrap) & | ^ << >> == != < <= > >= and or not (- x) (as T x) (mk S a b) (. s f) (f args)
MEM:  (alloc T n)->(buf T) zeroed  (get b i) (put b i v) (len b) (free b) (freeze b)->(ro T) shareable read-only
CONC: (spawn (f a))->(task R) (join t) | (group T) (go g (f a)) (wait g)->(buf T) in start order | (chan T cap) (send c v) (recv c) (close c) (each x c ...) | (pfor i lo hi ...) | (xadd b i v)->old (cas b i old new)->bool
IO:   (print a b ...) space-separated + newline; (clock) ns; (assert c)
EFFECTS: io=print/clock, mem=alloc/free, conc=spawn/go/wait/join/chan ops/pfor. Caller must declare every callee effect. Contracts pure.
OWNERSHIP: buf/task/group move, never copy. Moved var unusable. task/group must be joined/waited exactly once in owning scope, on every branch.
  fn returning buf/task/group OWNS its buf params (caller's var moves); otherwise it BORROWS (can get/put, cannot free/send/spawn them).
  Never move an outer linear var inside a loop. spawn/go move buf args. To get a buffer back from a task: return it.
PFOR: outer buf written only at loop var: (put b i v) + (get b i). Otherwise read-only, or atomics only. No set of outer vars, no break/ret.
ERRORS: JSON lines {code,line,col,msg,fix}. Apply `fix` when present. Traps: JSON on stderr, exit 101.
CLI: slopc check|run|build|test|fmt|sig|hash|emit-c|explain FILE  [--no-checks]

EXAMPLE
(fn count_primes ((n i64)) i64
  (eff mem conc)
  (let flags (alloc u8 n))
  (pfor i 0 n (if (is_prime i) (put flags i 1u8)))
  (var total 0)
  (for i 0 n (set total (+ total (as i64 (get flags i)))))
  (free flags)
  total)
(fn sum_range ((lo i64) (hi i64)) i64 (var s 0) (for i lo hi (set s (+ s i))) s)
(fn main () i64
  (eff io mem conc)
  (let g (group i64))
  (for k 0 4 (go g (sum_range (* k 100) (* (+ k 1) 100))))
  (let r (wait g))
  (print (get r 0) (get r 3))
  (free r)
  0)
