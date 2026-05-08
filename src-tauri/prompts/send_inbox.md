You will process a batch of customer feedback collected in this project's inbox. **STRICT RULE: do not modify any project code until I explicitly confirm.**

The inbox entry is at: `.ccpilot/inbox/{ENTRY_DIR}/`

## Step 1 — Read everything

Read ALL files under `.ccpilot/inbox/{ENTRY_DIR}/`:

- `feedback.md` — the title and the user's notes
- Any image files (screenshots) — view them
- Any other attachments (PDF, doc, etc.) — read them

Also read the project's `CLAUDE.md` at the project root if it exists. Then examine the project files that are most relevant to the feedback (use your judgment based on what the customer asks about).

## Step 2 — Write the distilled requirements

Write a clear, prioritized requirements list to `.ccpilot/inbox/{ENTRY_DIR}/request.md`. Format:

```markdown
# Requirements

## Confirmed
1. <thing the customer clearly wants>
2. ...

## Needs clarification
(if any — leave empty if none)
```

Cite file paths from the project where relevant.

## Step 3 — Ask focused questions for ANY ambiguity

For every uncertain interpretation, ask me ONE question at a time. Always include your recommended interpretation, with the code-based reason. Use this format:

```
Q: <restate the ambiguous bit of feedback>
推荐: <your interpretation>
理由: <which file/line led you to think so>
选项:
  A. <option A>
  B. <option B>
  其他: <free-form supplement>
```

Wait for my answer before asking the next question or moving on.

## Step 4 — Wait for confirmation

When all questions are resolved (or there were none), present:

```
我理解的需求如下：
1. ...
2. ...
是否准确？回复 yes 我开始改代码，或告诉我需要调整的地方。
```

**Do not touch project files until I reply yes.**

## Step 5 — Apply

Once confirmed, apply the changes precisely. Constraints:

- Stay strictly within the confirmed scope. No defensive refactors, no "while I'm here" cleanups.
- Don't introduce new dependencies unless explicitly requested.
- Don't change unrelated files.

After applying, briefly report:

- Files changed
- Any side effects or follow-ups needed
- Anything you intentionally did NOT change because it was out of scope

If at any point you discover that the requested change conflicts with existing code logic, pause and ask before making the change.
