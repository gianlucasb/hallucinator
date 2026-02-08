# Hallucinator TUI Design Document

> Design only. No implementation decisions about backend capabilities — if the
> backend can't support something yet, that's a separate problem.

## Who uses this and why

**Area chairs / senior PC members** reviewing 20-100 submissions for a venue.
They need to triage: which papers have suspicious references, and how
suspicious? They don't read every result — they scan for red flags, drill in
when something looks off, then move on.

**Reviewers** checking a handful of assigned papers (3-8 typically). More
likely to read results carefully. May re-run individual papers after authors
revise.

**Demo / conference hallway context.** Someone shows this to a colleague.
First impression matters — it should look like a tool built by someone who
gives a shit. But the flash has to be load-bearing: every visual element
should communicate something useful.

## Design principles

1. **Information density over decoration.** CS people read dense UIs
   comfortably. Don't waste space on padding when you could show data.
   Think Bloomberg terminal, not macOS settings.

2. **Glanceable status.** At any point you should be able to look at the
   screen for <1 second and know: how far along are we, is anything wrong,
   what needs my attention.

3. **Progressive disclosure.** Summary first, details on demand. The batch
   view shows paper-level status. Drill into a paper to see references.
   Drill into a reference to see per-database results.

4. **Don't block the user.** Analysis takes time. The user should be able
   to browse already-completed results while new papers are still running.

5. **Adaptive layout.** Must be usable at 80x24 (cramped but functional)
   and take advantage of 200+ column modern terminals. Not two separate
   layouts — one layout that flexes.

---

## Screens

There are three screens. You're always on exactly one.

### Screen 1: Queue

The entry point. Shows all papers (1 or 50) and their status.

```
 HALLUCINATOR                                          ██████░░░░ 12/50
─────────────────────────────────────────────────────────────────────────
 #   Paper                          Refs  ✓   ⚠   ✗   ☠   Status
─────────────────────────────────────────────────────────────────────────
  1  arxiv_2024_llm_survey.pdf       38  34   2   1   1   DONE
  2  neurips_submission_042.pdf       27  18   1   0   0   ████░░ 19/27
  3  review_response_v2.pdf          15   —   —   —   —   QUEUED
  4  transformer_scaling.pdf          —   —   —   —   —   QUEUED
  5  rlhf_safety_paper.pdf            —   —   —   —   —   QUEUED
 ...
 48  federated_privacy.pdf            —   —   —   —   —   QUEUED
 49  code_generation_bench.pdf        —   —   —   —   —   QUEUED
 50  multimodal_reasoning.pdf         —   —   —   —   —   QUEUED
─────────────────────────────────────────────────────────────────────────
 ✓ 34 verified  ⚠ 3 mismatch  ✗ 1 not found  ☠ 1 retracted     3:42 elapsed

 [enter] open  [r] retry failed  [d] delete  [a] add files  [q] quit
```

**Columns:**
- `#` — sequential, stable. Not the filename, because filenames are long.
- `Paper` — truncated filename. Full path shown on hover/focus.
- `Refs` — total reference count (blank until PDF is parsed).
- `✓ ⚠ ✗ ☠` — counts by verdict. These are the triage signal.
- `Status` — `QUEUED`, inline progress bar while running, `DONE` when
  finished. If errors occurred during parsing/extraction: `ERROR`.

**Behavior:**
- Papers are listed in queue order. Currently-running papers float to the
  top of the "not done" section (done papers above, then active, then
  queued).
- The cursor (highlighted row) selects a paper. Press `Enter` to go to
  Screen 2.
- While papers are running, counts update live. The overall progress bar
  in the header updates.
- Bottom row shows aggregate totals across all completed papers.

**Sorting:** Default is queue order. Allow re-sort by column (keybind or
click header). Most useful sort: by `✗` descending — puts the most
suspicious papers at top. A reviewer running 50 papers wants to see "which
5 papers have the most not-found references" immediately.

**Why this works for 1 paper:** If you pass a single PDF, this screen
still appears but with one row. It shows the progress bar filling up,
counts incrementing. When done, it auto-focuses that row so pressing
`Enter` takes you straight to the results. Feels natural, not like a
degenerate case of a batch view.

**Filtering:** Simple text filter on filename. Type `/` to start filtering
(vim convention). Also filter by status: e.g., `f` cycles through
"all → has problems → done → running → queued". The most common filter
is "show me only the papers that have problems."

---

### Screen 2: Paper

Shows all references for one paper and their verdicts.

```
 HALLUCINATOR > arxiv_2024_llm_survey.pdf               ██████████ 38/38
─────────────────────────────────────────────────────────────────────────
 #   Reference                                     Verdict     Source
─────────────────────────────────────────────────────────────────────────
  1  Vaswani et al. "Attention Is All You Need"    ✓ verified  arXiv
  2  Brown et al. "Language Models are Few-Shot..." ✓ verified  S2
  3  Wei et al. "Chain-of-Thought Prompting..."    ✓ verified  CrossRef
  4  Smith & Jones "Recursive Self-Improvement..." ✗ not found  —
  5  Zhang et al. "Emergent Abilities of..."       ⚠ mismatch  DBLP
  6  Chen et al. "Evaluating Large Language..."    ✓ verified  arXiv
 ...
 37  Wang et al. "Constitutional AI..."            ✓ verified  S2
 38  Davis "On the Retraction of..."               ☠ retracted CrossRef
─────────────────────────────────────────────────────────────────────────
 ✓ 34 verified  ⚠ 2 mismatch  ✗ 1 not found  ☠ 1 retracted

 [enter] details  [r] retry  [esc] back  [e] export  [s] sort
```

**Columns:**
- `#` — reference number as it appears in the paper.
- `Reference` — authors + truncated title. Quoted title portion to
  visually separate it from authors.
- `Verdict` — icon + word. Color-coded: green/yellow/red/magenta.
- `Source` — which database confirmed it (for verified) or `—` for not
  found. If multiple DBs confirmed, show the fastest one (the one that
  actually ended the search via early exit).

**Behavior:**
- If analysis is still running, references appear as they're processed.
  Unprocessed references show as dim/grey with `⏳ pending` or
  `⟳ checking` status.
- `Enter` on a reference opens Screen 3 (detail view).
- `r` on a specific reference retries just that one.
- `R` (shift) retries all failed/not-found references for this paper.

**Active reference animation:** The reference currently being checked gets
a subtle indicator — a spinner or a cycling set of dots. Nothing
aggressive. Just enough to show "this one is live." If multiple references
are being checked concurrently (which they are — 4 at a time), all active
ones show the indicator.

**Problem-first ordering:** Default sort is by reference number (paper
order). But `s` cycles through sort modes, and sort-by-verdict puts
not-found and retracted at the top. This is the thing the user actually
cares about — "show me the problems."

**Export:** `e` opens a small modal/prompt: export format (json / csv /
markdown / plain text) and destination (file path, clipboard). Exports
the results for this paper only. From the Queue screen, `e` exports all
papers.

---

### Screen 3: Reference Detail

Full detail on one reference. This is the "prove it" screen — when you
see a suspicious result you drill in here to understand why.

```
 HALLUCINATOR > arxiv_2024_llm_survey.pdf > [4]
─────────────────────────────────────────────────────────────────────────

 REFERENCE [4]
 Smith, J. and Jones, A. (2024)
 "Recursive Self-Improvement in Large Language Models:
  A Theoretical Framework"
 Proceedings of ICML 2024, pp. 1234-1248

 Verdict: ✗ NOT FOUND

 Extracted title:  "Recursive Self-Improvement in Large Language
                    Models: A Theoretical Framework"
 Extracted authors: J. Smith, A. Jones
 Extracted DOI:     none
 Extracted arXiv:   none

 DATABASE RESULTS
─────────────────────────────────────────────────────────────────────────
  Database       Result          Time     Notes
─────────────────────────────────────────────────────────────────────────
  CrossRef       no match        1.2s
  arXiv          no match        0.8s
  DBLP           no match        0.3s     (offline)
  Sem. Scholar   timeout         10.0s    retried: no match (12.4s)
  OpenAlex       no match        2.1s
  ACL            no match        0.4s
  NeurIPS        no match        0.6s
  Europe PMC     no match        1.8s
  PubMed         no match        0.9s
─────────────────────────────────────────────────────────────────────────

 No close matches found in any database.

 [r] retry  [c] copy ref text  [esc] back
```

For a **verified** reference, this screen would instead show:

```
 Verdict: ✓ VERIFIED (arXiv)

 Matched title:  "Attention Is All You Need"
 Match score:     98.2%
 Matched authors: Vaswani, Shazeer, Parmar, Uszkoreit, Jones, Gomez,
                  Kaiser, Polosukhin
 Author overlap:  8/8

 DATABASE RESULTS
─────────────────────────────────────────────────────────────────────────
  Database       Result          Time     Notes
─────────────────────────────────────────────────────────────────────────
  arXiv          ✓ match         0.3s     ← verified (early exit)
  CrossRef       (skipped)        —       early exit
  DBLP           (skipped)        —       early exit
  ...
```

For an **author mismatch**:

```
 Verdict: ⚠ AUTHOR MISMATCH (DBLP)

 Matched title:   "Emergent Abilities of Large Language Models"
 Match score:      96.1%
 Expected authors: Zhang, Wei, Chen
 Found authors:    Wei, Tay, Bommasani, Raffel, Zoph, Borgeaud,
                   Yogatama, Bosma, Zhou, Metzler, Chi, Hashimoto,
                   Vinyals, Liang, Dean, Fedus
 Author overlap:   1/3 (Wei)
```

**Why this screen matters:** "Not found" doesn't always mean hallucinated.
Maybe the title extraction mangled something. Maybe the paper is too new.
This screen lets a human make the judgment call by seeing exactly what
was searched for, what came back, and how long each database took. The
timing information helps distinguish "no match" from "everything timed
out" — very different confidence levels.

---

## Adaptive layout

### Narrow terminals (< 100 columns)

- Queue screen: hide `Refs` column, truncate filenames more aggressively,
  collapse `✓ ⚠ ✗ ☠` into a single "problems" count.
- Paper screen: hide `Source` column, truncate titles earlier.
- Detail screen: wraps naturally since it's mostly prose.

### Wide terminals (140+ columns)

- Queue screen: show full filenames, add an "elapsed time" column, add a
  "problems" column that sums ✗ + ☠ for quick scanning.
- Paper screen: show full titles without truncation, add a "time" column
  showing how long validation took per reference.
- Detail screen: split into two panes — reference info on the left,
  database results on the right (side-by-side instead of stacked).

### Very wide terminals (200+ columns)

- Queue screen: could show a mini-sparkline per paper showing
  distribution of verdicts as a tiny bar chart inline. Pure gravy.
- Paper screen: show the raw reference text in a right-side pane
  alongside the parsed/structured view. Useful for debugging extraction
  issues.

### Short terminals (< 30 rows)

- Collapse the header to a single line.
- Collapse the footer/keybinds bar to a single line.
- Use available rows for data.

---

## Live activity panel (overlay, not a screen)

Toggleable with `Tab`. This is the "flashy" part, but it earns its space.

When active, it takes the right 40-50 columns (or bottom third on narrow
terminals) and shows:

```
 ACTIVITY
────────────────────────────────
 Database Health
  CrossRef    ●  142ms avg
  arXiv       ●  89ms avg
  DBLP        ●  12ms avg  (offline)
  Sem.Scholar ◐  1.2s avg  throttled
  OpenAlex    ●  203ms avg
  ACL         ●  67ms avg
  NeurIPS     ●  94ms avg
  Europe PMC  ●  312ms avg
  PubMed      ○  down

 Rate Limits
  CrossRef   ░░░░▓▓░░░░  12/50
  S2         ░▓░░░░░░░░   3/100

 Throughput
  refs/min  ▁▂▃▅▇█▇▅▃▄▆█  avg: 8.2

 Active Queries
  → CrossRef: "Recursive Self-Imp..."
  → arXiv: "Recursive Self-Imp..."
  → DBLP: "Recursive Self-Imp..."
  ← S2: 429 Too Many Requests
```

**Database health indicators:**
- `●` — healthy (responding, <500ms average)
- `◐` — degraded (slow, rate limited, intermittent errors)
- `○` — down (repeated failures, all timeouts)

**Why this panel exists:** When you're running 50 papers, this answers
"why is it going slow" without you having to guess. If Semantic Scholar
is throttling you, you can see it. If PubMed is down, you know those
"not found" results are lower confidence. It converts backend
infrastructure state into visible, actionable information.

**Why it's an overlay and not a screen:** You want to see this *while*
browsing results. It's context, not content.

---

## Keyboard model

### Global (work on any screen)

| Key        | Action                                      |
|------------|---------------------------------------------|
| `q`        | Quit (confirms if analysis still running)   |
| `Tab`      | Toggle activity panel                       |
| `?`        | Toggle keybind help overlay                 |
| `Ctrl+C`   | Cancel current analysis / force quit        |

### Navigation

| Key            | Action                                  |
|----------------|-----------------------------------------|
| `↑/↓` `j/k`   | Move cursor                             |
| `Enter`        | Drill in (queue→paper→reference)        |
| `Esc`          | Back up one level                       |
| `g` / `G`      | Jump to top / bottom of list            |
| `Ctrl+D/U`     | Page down / page up                     |
| `/`            | Start text filter                       |
| `n` / `N`      | Next / previous filter match            |

### Actions

| Key        | Context        | Action                              |
|------------|----------------|-------------------------------------|
| `r`        | Queue          | Retry all failed refs in paper      |
| `r`        | Paper          | Retry selected reference             |
| `R`        | Paper          | Retry all failed refs in paper      |
| `r`        | Detail         | Retry this reference                 |
| `e`        | Queue          | Export all results                   |
| `e`        | Paper          | Export this paper's results          |
| `s`        | Queue / Paper  | Cycle sort mode                     |
| `f`        | Queue          | Cycle status filter                  |
| `a`        | Queue          | Add more files                      |
| `d`        | Queue          | Remove paper from queue             |
| `y`        | Detail         | Copy reference text to clipboard    |

### Mouse

- Click row to select.
- Double-click row to drill in.
- Click column header to sort.
- Scroll wheel scrolls the list.
- Click the `Tab` activity panel area to toggle it.

Not every action needs a mouse equivalent. Keyboard is the primary
interface. Mouse is a convenience for people who reach for it
instinctively.

---

## Color

The palette should work on both dark and light terminal backgrounds but
optimize for dark (that's what the target audience uses).

### Verdict colors

| Verdict         | Color          | Rationale                          |
|-----------------|----------------|------------------------------------|
| Verified        | Green (bold)   | Universal "good"                   |
| Author mismatch | Yellow         | Warning, needs human judgment      |
| Not found       | Red            | Danger / suspicious                |
| Retracted       | Magenta (bold) | Alarming, distinct from not-found  |
| Pending         | Dim / grey     | Not yet actionable                 |
| Checking        | Cyan           | Active, in-progress                |

### UI chrome

- Borders and separators: dim grey. Should recede.
- Headers/labels: white, bold.
- Selected row: reverse video (swap fg/bg). High contrast, works on any
  color scheme.
- Active database queries in activity panel: cyan.
- Rate limit bars: green → yellow → red gradient as capacity fills.

### Emphasis principle

Color is never the *only* signal. Every verdict also has a text label and
a distinct icon character (✓ ⚠ ✗ ☠). This matters for:
- Accessibility (color vision deficiency).
- Monochrome terminals / piped output.
- Screenshots in papers or blog posts that may be printed B&W.

---

## Startup sequence

```
$ hallucinator ~/papers/*.pdf

 ░█░█░█▀█░█░░░█░░░█░█░█▀▀░▀█▀░█▀█░█▀█░▀█▀░█▀█░█▀▄
 ░█▀█░█▀█░█░░░█░░░█░█░█░░░░█░░█░█░█▀█░░█░░█░█░█▀▄
 ░▀░▀░▀░▀░▀▀▀░▀▀▀░▀▀▀░▀▀▀░▀▀▀░▀░▀░▀░▀░░▀░░▀▀▀░▀░▀

 Loading 50 PDFs...
 Databases: CrossRef arXiv DBLP(offline) S2 OpenAlex ACL NeurIPS PMC PubMed
```

Brief. The banner renders instantly (no animation — animation on startup
is a delay). The database line confirms which sources are enabled and
shows if DBLP is running in offline mode. Then it transitions to the Queue
screen within ~1 second as PDFs start parsing.

If the banner won't fit (terminal < 70 columns), skip it and go straight
to the Queue screen.

---

## The "50 papers at 2am" workflow

This is the scenario that matters most. An area chair has a deadline.
They run:

```
$ hallucinator ~/openreview-downloads/*.pdf
```

**First 10 seconds:** Queue screen populates with 50 filenames. First
paper starts processing. Activity panel shows databases warming up.

**Next few minutes:** Papers process. The user watches for a bit,
sees the system is working, then does something else in another terminal
tab.

**They come back:** 35 papers done. They press `s` to sort by problems.
The 3 papers with not-found references float to the top. They press
`Enter` on the worst one, see 4 not-found references, drill into each
to see what the tool searched for. Two look like genuine hallucinations
(zero matches across all 9 databases). Two look like very recent
preprints that just aren't indexed yet (only in arXiv, which timed out).

They press `Esc` back to Queue, check the next problem paper. After
5 minutes of triage they have a clear picture: 2 submissions with
probable fabricated references, 1 with a retracted citation the authors
should have caught.

They press `e`, export a JSON report of all results, and attach it to
their AC notes.

**What mattered:** Sort by problems. Fast drill-in/drill-out. Export.
Not the sparklines or the database race visualization — those were nice
for the first 30 seconds but the actual utility is in triage speed.

---

## Non-goals for TUI

- **Configuration editing.** Don't build a settings screen. API keys, DB
  toggles, timeouts — these are CLI flags and env vars. The TUI is for
  analysis, not setup.
- **PDF viewing.** Don't try to render the paper. Users have their own
  PDF viewer open alongside.
- **Editing results.** The TUI is read-only for results. No "mark as
  false positive" or annotation features. That's a different tool.
- **Log viewer.** The activity panel is not a log. Don't show every HTTP
  request. Show *state* (database health, rate limits, throughput) not
  *events*.

---

## Visual mockups: states and scenarios

### Queue screen — mid-run, sorted by problems

The area chair has been running for a few minutes and just hit `s` to
sort by descending problem count.

```
 HALLUCINATOR                                          ████████░░ 41/50
─────────────────────────────────────────────────────────────────────────
 #   Paper                          Refs  ✓   ⚠   ✗   ☠   Status
─────────────────────────────────────────────────────────────────────────
  7  sketchy_submission_v3.pdf        22  14   1   4   1   DONE
 31  workshop_paper_draft.pdf         18  11   2   3   0   DONE
 12  llm_alignment_study.pdf          35  28   3   2   0   DONE
  1  arxiv_2024_llm_survey.pdf        38  34   2   1   1   DONE
 19  multiagent_reasoning.pdf         29  27   1   1   0   DONE
  3  safety_evaluation.pdf            41  39   2   0   0   DONE
  5  federated_learning.pdf           33  33   0   0   0   DONE
 ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─
 42  code_gen_benchmark.pdf           27  18   —   —   —   █████░ 18/27
 43  vision_transformer.pdf           19   —   —   —   —   PARSING
 ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─
 44  diffusion_models.pdf              —   —   —   —   —   QUEUED
 45  robustness_theory.pdf             —   —   —   —   —   QUEUED
 ...8 more
─────────────────────────────────────────────────────────────────────────
 ✓ 412 verified  ⚠ 18 mismatch  ✗ 11 not found  ☠ 2 retracted  12:34

 sorted: problems ↓          [enter] open  [s] sort  [f] filter  [q] quit
```

Note the three visual zones separated by dashed rules: done (sorted),
active (running now), and queued. The user's eye goes straight to the top
— paper #7 with 4 not-found and 1 retracted is the one to investigate.

### Queue screen — narrow terminal (80 columns)

Same data, collapsed for a small terminal:

```
 HALLUCINATOR                        ████████░░ 41/50
────────────────────────────────────────────────────────
  #  Paper                     Probs  Status
────────────────────────────────────────────────────────
   7 sketchy_submission_v3…     5     DONE
  31 workshop_paper_draft…      3     DONE
  12 llm_alignment_study…       2     DONE
   1 arxiv_2024_llm_surv…       2     DONE
  19 multiagent_reasoning…      1     DONE
   3 safety_evaluation…         0     DONE
   5 federated_learning…        0     DONE
  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─
  42 code_gen_benchmark…        —     █████░ 18/27
  43 vision_transformer…        —     PARSING
  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─
  44 diffusion_models…          —     QUEUED
  ...
────────────────────────────────────────────────────────
 [enter] open  [s] sort  [f] filter  [q] quit
```

Individual verdict columns collapse into a single "Probs" count
(✗ + ☠ + ⚠). Still scannable. The user loses per-type breakdown at a
glance but gains it back by drilling in.

### Queue screen — wide terminal with activity panel (180+ columns)

```
 HALLUCINATOR                                          ████████░░ 41/50         │ ACTIVITY
──────────────────────────────────────────────────────────────────────────────────┤────────────────────────────────────
 #   Paper                              Refs  ✓   ⚠   ✗   ☠   Time   Status   │ Database Health
──────────────────────────────────────────────────────────────────────────────────│  CrossRef    ●  142ms
  7  sketchy_submission_v3.pdf            22  14   1   4   1   0:48   DONE     │  arXiv       ●   89ms
 31  workshop_paper_draft.pdf             18  11   2   3   0   0:35   DONE     │  DBLP        ●   12ms  offline
 12  llm_alignment_study.pdf              35  28   3   2   0   1:12   DONE     │  Sem.Scholar ◐  1.2s   throttled
  1  arxiv_2024_llm_survey.pdf            38  34   2   1   1   1:31   DONE     │  OpenAlex    ●  203ms
 19  multiagent_reasoning.pdf             29  27   1   1   0   0:55   DONE     │  ACL         ●   67ms
  3  safety_evaluation.pdf                41  39   2   0   0   1:44   DONE     │  NeurIPS     ●   94ms
  5  federated_learning.pdf               33  33   0   0   0   1:22   DONE     │  Europe PMC  ●  312ms
 ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─│  PubMed      ○  down
 42  code_gen_benchmark.pdf               27  18   —   —   —   0:22   ████░    │
 43  vision_transformer.pdf               19   —   —   —   —    —     PARSING  │ Rate Limits
 ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─│  CrossRef  ░░░▓▓▓░░░░  18/50
 44  diffusion_models.pdf                  —   —   —   —   —    —     QUEUED   │  S2        ░▓▓▓▓▓▓▓░░  71/100
 45  robustness_theory.pdf                 —   —   —   —   —    —     QUEUED   │
 ...8 more                                                                      │ Throughput (refs/min)
──────────────────────────────────────────────────────────────────────────────────│  ▁▂▃▅▇█▇▅▃▁▂▅▇█▆▅  avg: 8.2
 ✓ 412  ⚠ 18  ✗ 11  ☠ 2                                           12:34       │
 sorted: problems ↓    [enter] open  [s] sort  [f] filter  [Tab] panel  [q] quit│
```

The activity panel earns its space here. You can see S2 is almost at its
rate limit (71/100) — that's why it shows as "throttled." PubMed is
straight up down. The throughput sparkline shows a dip about 2 minutes
ago (probably when S2 started throttling) and a recovery.

### Paper screen — actively checking, with activity panel

Drilled into paper #42 which is still running:

```
 HALLUCINATOR > code_gen_benchmark.pdf                   █████░░░░ 18/27        │ ACTIVITY
──────────────────────────────────────────────────────────────────────────────────┤──────────────────────────────
 #   Reference                                     Verdict      Source          │ Database Health
──────────────────────────────────────────────────────────────────────────────────│  CrossRef    ●  142ms
  1  Chen et al. "Evaluating Large Language..."    ✓ verified   arXiv           │  arXiv       ●   89ms
  2  Austin et al. "Program Synthesis with..."     ✓ verified   S2              │  DBLP        ●   12ms  offline
  3  Li et al. "Competition-Level Code..."         ✓ verified   CrossRef        │  Sem.Scholar ◐  1.2s
  4  Hendrycks et al. "Measuring Coding..."        ✓ verified   CrossRef        │
  5  Nijkamp et al. "CodeGen: An Open..."          ✓ verified   S2              │ Active Now
 ...                                                                            │  [19] → CrossRef  ⟳
 17  Fried et al. "InCoder: A Generative..."       ✓ verified   DBLP            │  [19] → arXiv     ⟳
 18  Allal et al. "SantaCoder: Don't..."           ✓ verified   S2              │  [19] → DBLP      ✓ 12ms
────────────────────────────────────────────────────────────────────────────────  │  [19] → S2        ⟳
 19  Wang et al. "Execution-Based Code..."         ⟳ checking   3/9             │  [20] → CrossRef  ⟳
 20  Fake et al. "An Invented Paper..."            ⟳ checking   1/9             │  [20] → arXiv     waiting
 21  Zhang et al. "RepoCoder: Repository..."       ⟳ checking   0/9             │  [21] → queued
 22  Liu et al. "Is Your Code Generated..."        ⏳ pending                    │  [22] → queued
 ...                                                                            │
 27  Peng et al. "The Impact of AI on..."          ⏳ pending                    │
──────────────────────────────────────────────────────────────────────────────────│
 ✓ 18 verified  so far                                                          │
 [enter] details  [r] retry  [esc] back  [s] sort                              │
```

The activity panel here shows per-query granularity: reference [19] has
3 databases done (DBLP already returned a match at 12ms but it's still
waiting on others — or maybe early exit will kick in and cancel the
remaining). Reference [20] just started. [21] and [22] are queued
waiting for a slot in the concurrency pool.

This is the "database race" made visible. You're watching 4 references
being checked concurrently, each with up to 9 databases racing. When a
database returns a match, the whole reference can resolve instantly via
early exit.

### Paper screen — done, filtered to problems only

After analysis completes, the reviewer presses `f` to filter to problems:

```
 HALLUCINATOR > sketchy_submission_v3.pdf                  DONE ✓14 ⚠1 ✗4 ☠1
─────────────────────────────────────────────────────────────────────────
 #   Reference                                     Verdict      Source
─────────────────────────────────────────────────────────────────────────
  3  Zhang & Li "Self-Aware Neural..."             ✗ not found   —
  8  Johnson et al. "Recursive Prompt..."          ✗ not found   —
 11  Chen "Advanced Reasoning in..."               ✗ not found   —
 15  Park et al. "Constitutional Self-..."         ✗ not found   —
 19  Davis et al. "On the Emergent..."             ☠ retracted  CrossRef
  6  Williams et al. "Scaling Laws for..."         ⚠ mismatch   DBLP
─────────────────────────────────────────────────────────────────────────
 showing: problems only (6/22)

 [enter] details  [f] show all  [r] retry  [e] export  [esc] back
```

Six references instead of 22. The reviewer only needs to look at these.
Four not-found references in a 22-reference paper is a strong signal.

### Reference detail — close match found but authors wrong

```
 HALLUCINATOR > sketchy_submission_v3.pdf > [6]
─────────────────────────────────────────────────────────────────────────

 REFERENCE [6]
 Williams, R., Thompson, K., and Garcia, M. (2023)
 "Scaling Laws for Neural Language Models"
 In Proceedings of NeurIPS 2023

 Verdict: ⚠ AUTHOR MISMATCH

 Extracted title:   "Scaling Laws for Neural Language Models"
 Extracted authors:  R. Williams, K. Thompson, M. Garcia

 BEST MATCH (CrossRef)
─────────────────────────────────────────────────────────────────────────
  Matched title:    "Scaling Laws for Neural Language Models"
  Title score:       100.0%
  Found authors:     Jared Kaplan, Sam McCandlish, Tom Henighan,
                     Tom B. Brown, Benjamin Chess, Rewon Child,
                     Scott Gray, Alec Radford, Jeffrey Wu, Dario Amodei
  Author overlap:    0/3 — no matching authors

  DOI:               10.48550/arXiv.2001.08361
  Source:            CrossRef (0.8s)
─────────────────────────────────────────────────────────────────────────

 This paper exists but the cited authors (Williams, Thompson, Garcia)
 don't match the actual authors (Kaplan, McCandlish, et al.).

 ALL DATABASE RESULTS
─────────────────────────────────────────────────────────────────────────
  CrossRef       ⚠ author mismatch  0.8s   (match shown above)
  arXiv          ⚠ author mismatch  0.4s
  DBLP           ⚠ author mismatch  0.1s   (offline)
  Sem. Scholar   timeout            10.0s
  OpenAlex       ⚠ author mismatch  1.2s
  ACL            no match           0.3s
  NeurIPS        no match           0.5s
  Europe PMC     no match           1.1s
  PubMed         no match           0.7s
─────────────────────────────────────────────────────────────────────────

 [r] retry  [y] copy ref text  [esc] back
```

This is a telltale sign: the paper "Scaling Laws for Neural Language
Models" is real (Kaplan et al., 2020), but the submission attributed it
to completely fabricated authors. Four databases independently confirmed
the title exists with different authors. The detail screen makes this
case unambiguous.

### Reference detail — retracted paper

```
 HALLUCINATOR > sketchy_submission_v3.pdf > [19]
─────────────────────────────────────────────────────────────────────────

 REFERENCE [19]
 Davis, P., Reeves, L., and Kang, S. (2021)
 "On the Emergent Properties of Transformer Architectures
  in Low-Resource Settings"
 Journal of Machine Learning Research, 22(1), pp. 1-34

 Verdict: ☠ RETRACTED

 Extracted title:   "On the Emergent Properties of Transformer
                     Architectures in Low-Resource Settings"
 Extracted authors:  P. Davis, L. Reeves, S. Kang

 MATCH (CrossRef)
─────────────────────────────────────────────────────────────────────────
  Matched title:    "On the Emergent Properties of Transformer
                     Architectures in Low-Resource Settings"
  Title score:       100.0%
  Found authors:     P. Davis, L. Reeves, S. Kang
  Author overlap:    3/3 ✓

  DOI:               10.xxxx/jmlr.2021.xxxxx
  Source:            CrossRef (1.1s)

  ╔══════════════════════════════════════════════════════════════════╗
  ║  ☠ RETRACTION NOTICE                                           ║
  ║                                                                 ║
  ║  This paper was retracted on 2022-03-15.                       ║
  ║  Retraction DOI: 10.xxxx/jmlr.2022.retract.xxxxx              ║
  ║  Reason: "Results could not be reproduced; data fabrication     ║
  ║  suspected."                                                    ║
  ╚══════════════════════════════════════════════════════════════════╝
─────────────────────────────────────────────────────────────────────────

 [r] retry  [y] copy ref text  [esc] back
```

The retraction notice gets a heavy box border — it's the most important
piece of information on this screen and should be impossible to miss.

### Single-paper mode — just started

When invoked with a single PDF:

```
 HALLUCINATOR
─────────────────────────────────────────────────────────────────────────
 arxiv_2024_llm_survey.pdf                         38 references found
─────────────────────────────────────────────────────────────────────────
  1  Vaswani et al. "Attention Is All You Need"    ✓ verified   arXiv
  2  Brown et al. "Language Models are Few-..."    ✓ verified   S2
  3  Wei et al. "Chain-of-Thought Prompting..."    ✓ verified   CrossRef
  4  Bubeck et al. "Sparks of Artificial..."       ✓ verified   S2
  5  Touvron et al. "LLaMA: Open and..."           ⟳ checking   4/9
  6  Chowdhery et al. "PaLM: Scaling..."           ⟳ checking   2/9
  7  Hoffmann et al. "Training Compute-..."        ⟳ checking   0/9
  8  Ouyang et al. "Training language..."          ⟳ checking   0/9
  9  Bai et al. "Constitutional AI:..."            ⏳ pending
 10  Raffel et al. "Exploring the Limits..."       ⏳ pending
 ...
 38  Kojima et al. "Large Language Models..."      ⏳ pending
─────────────────────────────────────────────────────────────────────────
 ████░░░░░░ 4/38    ✓ 4 verified                            0:12

 [enter] details  [Tab] activity  [q] quit
```

No queue screen — it drops you directly into the paper view. The
progress bar and running counts update live. This feels immediate and
purposeful. When it finishes, the status line updates and you can browse
results or export.

### Export modal

Pressing `e` on any screen:

```
         ┌─ Export ──────────────────────────────┐
         │                                       │
         │  Format:  [JSON]  CSV  Markdown  Text │
         │  Scope:   This paper / All papers     │
         │  Output:  ~/hallucinator-results.json │
         │                                       │
         │          [Export]    [Cancel]          │
         └───────────────────────────────────────┘
```

Minimal modal. Arrow keys or tab to move between options. Enter to
confirm. Esc to cancel. The output path has a sensible default and is
editable.

### Help overlay

Pressing `?` on any screen:

```
 ┌─ Keybindings ─────────────────────────────────────────────────┐
 │                                                               │
 │  Navigation                     Actions                      │
 │  ↑↓ j/k    move cursor          r    retry reference/paper   │
 │  Enter      drill in             R    retry all failed        │
 │  Esc        back                 e    export results          │
 │  g/G        top/bottom           s    cycle sort mode         │
 │  Ctrl+D/U   page down/up         f    cycle filter            │
 │  /          search/filter        a    add files (queue)       │
 │  n/N        next/prev match      d    remove paper (queue)    │
 │                                  y    copy to clipboard       │
 │  Global                                                       │
 │  Tab        toggle activity      ?    this help               │
 │  q          quit                 Ctrl+C  cancel/force quit    │
 │                                                               │
 │                                             [?/Esc] close     │
 └───────────────────────────────────────────────────────────────┘
```

Semi-transparent overlay on top of whatever screen is active. The
underlying screen is still visible (dimmed) so you maintain spatial
context.

## Decisions (resolved)

### 1. Notification on completion

Terminal bell by default. Works everywhere, zero config. Desktop
notification via `notify-send` / platform equivalent available as
opt-in flag (`--notify`). Don't overthink this.

### 2. Results persistence

Two distinct mechanisms:

**Temp state (invisible infrastructure).** In-progress and completed
results write to `~/.cache/hallucinator/runs/<timestamp>/`. This is
crash safety — if the terminal dies, SSH drops, or the user hits
`Ctrl+C`, the work isn't lost. The TUI doesn't expose this to the
user. It just exists.

**Export (deliberate user action).** `e` key opens the export modal.
User picks format (JSON, CSV, Markdown, plain text), scope (one paper
or all), and destination. This produces the actual deliverable — the
report they attach to AC notes or share with co-reviewers.

**Resume (future).** Not in v1. Eventually: `hallucinator --resume`
reads from the temp state dir and picks up where it left off. The temp
state format should be designed with this in mind even if we don't
build the resume path yet — don't paint ourselves into a corner.

### 3. Reference text preview pane

Yes. Shown on the Paper screen (Screen 2) when terminal height >= 40
rows. Located below the reference list, separated by a horizontal rule.
Shows the raw reference text as extracted from the PDF for the
currently-selected reference.

Updates in real-time as the cursor moves through the reference list
(file-manager-style preview). This is the expected behavior and the
rendering cost is trivial — it's just text reflow.

On terminals shorter than 40 rows, the preview is hidden. The user can
still see the raw text by drilling into Screen 3.

```
 ...
  4  Smith & Jones "Recursive Self-Imp..."  ✗ not found   —
> 5  Zhang et al. "Emergent Abilities..."   ⚠ mismatch   DBLP
  6  Chen et al. "Evaluating Large..."      ✓ verified   arXiv
 ...
─────────────────────────────────────────────────────────────────
 [5] Zhang, W., Wei, J., and Chen, L., "Emergent Abilities of
 Large Language Models," in Proceedings of the International
 Conference on Machine Learning (ICML), 2023, pp. 4812-4830.
─────────────────────────────────────────────────────────────────
```

This earns its space. When the extracted title looks wrong (mangled by
hyphenation, ligature issues, or a bad parse), you see it instantly
without an extra keypress.

### 4. Color themes

Two themes, toggled via `--theme=green` or `--theme=modern`. No
theming framework, no `theme.toml`. Just two palette structs.

**green (default):** Dark background, green/cyan primary text. Terminal
hacker aesthetic. The one that makes people at poster sessions say
"what is that." Verdict colors as specified in the Color section above.

**modern:** Dark background, white primary text, electric blue accents.
Cleaner, more subdued. For people who think the green is too much, or
for screenshots in formal reports where neon green looks unserious.

Both palettes follow the same rules: verdict colors stay semantically
consistent (green=verified, red=not found, etc.), only the chrome and
accent colors differ.

### 5. Inline retry feedback

Both spinner and text. The verdict cell shows an animated spinner
character cycling through frames (`◜ ◝ ◞ ◟`) followed by static
"retrying" text:

```
  4  Smith & Jones "Recursive Self-..."    ◝ retrying    —
```

The spinner provides motion ("something is happening") while the text
provides meaning ("what is happening"). Consistent with how the
`⟳ checking` state already works during initial analysis — just a
different animation to distinguish retry from first pass.

When the retry completes, the cell snaps to the new verdict. No
transition animation — just the immediate update. The change in color
(from cyan retrying to green/red/yellow result) is transition enough.
