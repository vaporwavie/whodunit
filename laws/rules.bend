import Base

# How a run ends: /done's score folds into the outcome the scoreboard counts.
type Outcome is Data:
  Failed{}
  Fixes{}
  Nailed{}

def fold(s: Nat) -> Outcome:
  match s:
    case 0n:
      Failed{}
    case 1n:
      Failed{}
    case 2n:
      Fixes{}
    case 3n:
      Fixes{}
    case 4n+p:
      Nailed{}

def rank(o: Outcome) -> Nat:
  match o:
    case Failed{}:
      0n
    case Fixes{}:
      1n
    case Nailed{}:
      2n

type Ending is Data:
  Ending{score: Nat, outcome: Outcome}

def finish(switched: Bool, +s: Nat) -> Ending:
  match switched:
    case True{}:
      Ending{1n, Failed{}}
    case False{}:
      Ending{s, fold(s)}

# What the UI may show of a deal: nothing until the run has ended.
type Deal is Data:
  Deal{role: Nat, agent: Nat}

def shown(deal: Deal, ended: Maybe<&2, Nat>) -> Maybe<&2, Deal>:
  match ended:
    case None{}:
      None{}
    case Some{s}:
      Some{deal}

# The deck: contenders as numbers, each cycle's shuffle passed in from outside.
# deal_n(n, deck, shuffle) deals n cards; an empty deck refills from the shuffle.
type Hand is Data:
  Hand{dealt: List<&2, Nat>, left: List<&2, Nat>}

def add(card: Nat, hand: Hand) -> Hand:
  match hand:
    case Hand{dealt, left}:
      Hand{card <> dealt, left}

def deal_n(n: Nat, deck: List<&2, Nat>, +shuffle: List<&2, Nat>) -> Hand:
  match n deck:
    case 0n d:
      Hand{Nil{}, d}
    case 1n+k h <> t:
      add(h, deal_n(k, t, shuffle))
    case 1n+k Nil{}:
      match shuffle:
        case Nil{}:
          Hand{Nil{}, Nil{}}
        case h <> t:
          add(h, deal_n(k, t, shuffle))
