# Results

The first trial ran from 2026-11-08 to 2026-11-11: 65 rated runs and 3 fan-outs, $147 at list price. The setup and the contenders are in the [README](README.md#the-contenders).

## By role

| Role | Winner | Result |
|---|---|---|
| `builder` | **Opus 5.5** | 4.89 against GPT-6.1 Sol's 4.50 over 18 runs each, at three times the cost per run |
| `brainiac` | **Opus 5.5** | Tied with GPT-6 Astra at 5.00 (7 and 9 runs), and cheaper per run, $3.39 against $3.57 |
| `errand-boy` | **Kimi K3.1 Pro** | The only contender with three runs at 5.00. Nobody failed a chore, and GPT-6.1 Sol did them cheapest |
| `scout` | none | 2 fan-outs, one each to Opus 5.5 and GPT-6 Astra |
| `scribe` | none | 1 fan-out, won by Gemini 4 Argon |

A winner needs at least 3 rated runs, or 3 fan-outs in a fan-out role. Ties go to the cheaper agent. Those are the [scoreboard's rules](README.md#scoreboard), set before the trial started.

## Winners

**Opus 5.5 wins the experiment.** It took two of the three roles that had enough runs to call, and it never scored below a 4 in 28 rated runs: 26 fives and 2 fours, a 4.93 mean. That is the result I guessed in the README before I started. The trial confirmed it and put a price on it.

**`builder`: Opus 5.5, and this is the one clear win.** Both contenders got 18 rated runs. Opus scored sixteen 5s and two 4s. GPT-6.1 Sol scored twelve 5s, three 4s and three 3s, so 3 of its 18 runs needed my fixes and none of Opus's did. The median Opus run took 0.0 minutes of my time between turns, and the median Sol run took 4.0. Opus costs $2.74 a run against Sol's $0.89.

**`brainiac`: Opus 5.5, on the tie-break only.** Opus and GPT-6 Astra scored a 5 on every run, 7 and 9 of them. Opus wins because it was 18 cents cheaper per run. On quality I can't tell them apart.

**`errand-boy`: Kimi K3.1 Pro, by a thin margin.** Kimi went 5, 5, 5. Gemini 4 Argon and Opus also scored 5.00, but over 2 runs each, one short of counting. GPT-6.1 Sol had its 3 runs and dropped one point, 4.67. Every contender nailed every chore, so the role is really decided by price: Sol at $0.12 a run, Gemini at $0.35, Kimi at $0.65, Opus at $1.41.

**Best value: GPT-6.1 Sol.** It is the cheapest contender in both roles it played, a third of Opus's cost in `builder` and a twelfth in `errand-boy`, and it nailed 18 of 21 runs.

**The underdogs held up.** Kimi K3.1 Pro and Gemini 4 Argon scored a 5 on all five of their `errand-boy` runs, and Gemini won the only `scribe` fan-out at 4 cents. Kimi lost both `scout` fan-outs and took about twice as long as the others in each.

## What the trial can't say

- 56 of the 65 runs scored a 5, so the 5-to-1 card separated the models less than I expected. Only `builder` produced a gap.
- `scout` and `scribe` got 3 fan-outs between them, too few to name a winner.
- The trial is two days of my own tasks, 63 of the runs on 2026-11-09 and 2026-11-10.
- One Opus run on Linux is left out because it failed on a bad API key before reaching the model.
