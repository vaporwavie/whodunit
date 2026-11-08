import Base
import ./rules.bend as R
import ./LAWS.bend as Laws

def Laws.five_is_nailed():
  {==}

def Laws.one_is_failed():
  {==}

def Laws.fold_monotone(s):
  match s:
    case 0n:
      {==}
    case 1n:
      {==}
    case 2n:
      {==}
    case 3n:
      {==}
    case 4n+p:
      {==}

def Laws.switch_fails(s):
  {==}

def Laws.score_kept(s):
  {==}

def Laws.hidden_until_ended(d):
  {==}

def Laws.shown_once_ended(d, s):
  {==}

def Laws.no_early_refill(h, t, p):
  {==}

def Laws.drain(d, p):
  match d:
    case Nil{}:
      {==}
    case h <> t:
      e = Laws.drain(t, p)
      %Equal.sym(R.Hand, R.deal_n(List.length(&2, Nat, t), t, p), R.Hand{t, Nil{}}, e) : {R.add(h, _) == R.Hand{h <> t, Nil{}} : R.Hand}
      {==}

def Laws.cycle(p):
  match p:
    case Nil{}:
      {==}
    case h <> t:
      e = Laws.drain(t, h <> t)
      %Equal.sym(R.Hand, R.deal_n(List.length(&2, Nat, t), t, h <> t), R.Hand{t, Nil{}}, e) : {R.add(h, _) == R.Hand{h <> t, Nil{}} : R.Hand}
      {==}
