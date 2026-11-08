import Base
import ./rules.bend as R

# Ending a run

law five_is_nailed:
  {R.fold(5n) == R.Nailed{} : R.Outcome}

law one_is_failed:
  {R.fold(1n) == R.Failed{} : R.Outcome}

law fold_monotone:
  for s: Nat
  {Nat.is_le(R.rank(R.fold(s)), R.rank(R.fold(1n+s))) == True{} : Bool}

law switch_fails:
  for +s: Nat
  {R.finish(True{}, s) == R.Ending{1n, R.Failed{}} : R.Ending}

law score_kept:
  for +s: Nat
  {R.finish(False{}, s) == R.Ending{s, R.fold(s)} : R.Ending}

# Hidden until the run ends

law hidden_until_ended:
  for d: R.Deal
  {R.shown(d, None{}) == None{} : Maybe<&2, R.Deal>}

law shown_once_ended:
  for +d: R.Deal
  for s: Nat
  {R.shown(d, Some{s}) == Some{d} : Maybe<&2, R.Deal>}

# The deck

law no_early_refill:
  for h: Nat
  for t: List<&2, Nat>
  for +p: List<&2, Nat>
  {R.deal_n(1n, h <> t, p) == R.Hand{h <> Nil{}, t} : R.Hand}

law drain:
  for +d: List<&2, Nat>
  for +p: List<&2, Nat>
  {R.deal_n(List.length(&2, Nat, d), d, p) == R.Hand{d, Nil{}} : R.Hand}

law cycle:
  for +p: List<&2, Nat>
  {R.deal_n(List.length(&2, Nat, p), Nil{}, p) == R.Hand{p, Nil{}} : R.Hand}
