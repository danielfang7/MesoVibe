# TODOS

## EXP bar reader

### A stray pixel over a low EXP digit can be learned as a digit not seen yet

**What:** Stop `learn_unseen_digit` from naming a stray-pixel shape (the game's cursor) as the one missing digit from the full EXP field ([#37](https://github.com/danielfang7/MesoVibe/issues/37)).

**Why:** The check it relies on, the EXP fitting the level's EXP needed, tolerates about a hundredth of a percent, so a pixel over a low digit passes. The reading is then confirmed wrong, and the bad shape is saved for later sessions.

**Context:** `engine/src/recognizers/exp_bar.rs`, `learn_unseen_digit`. The same check is on `main`; review of the percent-only work found it. Learning from the percent alone was removed for the same reason. Start by requiring the same shape to fit in two different frames.

**Effort:** S
**Priority:** P1
**Depends on:** None

### One frame with the level hidden resets the worked-out EXP and logs a fake EXP loss

**What:** Keep the percent-only EXP estimate anchored to the last reading that had an EXP, so a frame with the level tiles covered doesn't reset it ([#38](https://github.com/danielfang7/MesoVibe/issues/38)).

**Why:** The reset moves the EXP by up to half a hundredth of a percent of the level (about 54 EXP at level 58), and the Session Tracker logs it as a death.

**Context:** `engine/src/recognizers/exp_bar.rs`, `estimate`, called from `observe_found`. Reproduced at level 41: 115,705 became 115,689 at the same 35.68% after one hidden-tiles frame.

**Effort:** S
**Priority:** P1
**Depends on:** None

### A misread first full EXP field at a level blanks the EXP once only the percent shows

**What:** Treat a level's first full-field reading that doesn't fit the EXP table as a possible misread, the way a later one is treated ([#39](https://github.com/danielfang7/MesoVibe/issues/39)).

**Why:** Today such a reading marks the level's row disproved, so the EXP and its events stay blank for the rest of the level once the HUD shows only the percent.

**Context:** `engine/src/recognizers/exp_bar.rs`, `follow_level`, where it pushes a new `RowCheck`.

**Effort:** S
**Priority:** P2
**Depends on:** None

## Completed
