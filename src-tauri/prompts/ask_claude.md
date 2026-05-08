You are creating a script in this project's `.ccpscript/` directory based on the user's specific instruction.

## User instruction

{USER_INSTRUCTION}

## Task

1. **Focus on the user's request.** Generate ONLY the script(s) that fulfill it. Do **not** auto-generate every possible script like detect mode does.
2. Filename should reflect the purpose, lowercase with hyphens (e.g., `docker-prod.ps1`, `migrate-dev.ps1`).
3. Each script's first line MUST be `# Title: <concise title>`.
4. Keep script content concise. One line when possible; split into multiple lines for prerequisites (cd, env switch, etc.).
5. Usually 1 script per request is enough. If the request clearly covers multiple related scenarios (e.g. "set up a docker workflow"), produce at most 2–3 scripts.

## Critical: lightweight static checks

After each script is generated, run these three checks. **Do NOT actually run business commands**.

### Step 1: PowerShell syntax parse (no execution)

```powershell
powershell -NoProfile -Command "$null = [ScriptBlock]::Create([System.IO.File]::ReadAllText('.ccpscript/<name>.ps1'))"
```

Exit code 0 = syntax valid; non-zero = the script has a typo. **Fix the script, retry at most once.**

### Step 2: Command reference verification

For each command in the script, verify references exist:

- Contains `npm run X` / `pnpm X` / `yarn X` / `bun X` → read `package.json`, verify `scripts.X` actually exists
- Contains `cargo X` → verify `Cargo.toml` exists
- Contains `docker compose ...` or `docker-compose ...` → verify `docker-compose.yml` / `compose.yaml` exists
- Contains `python X.py` or `python -m Y` → verify the file/module exists
- Contains `go run` / `go build` → verify `go.mod` exists
- Contains `Set-Location X` / `cd X` (relative path) → verify the subdirectory exists
- Explicitly references `.env` or config files → verify they exist

Any hard error → **fix the script, retry at most once**.

### Step 3: Tool PATH check

For each main external command:

```powershell
Get-Command <tool> -ErrorAction SilentlyContinue
```

If empty → don't delete the script, add a comment at the top noting the tool wasn't on PATH.

## Don't do

- ❌ Don't actually start a dev server / `docker compose up`
- ❌ Don't actually run build / test
- ❌ Don't run `npm install`
- ❌ Don't spend more than 5 seconds on a single script verification
- ❌ Don't generate scripts beyond what the user asked for

## Key principles

- Stick strictly to the user's request — don't expand scope
- Checks only catch "obvious mistakes"
- If verification fails and one retry doesn't fix it → delete that script
- When done, briefly state what you did and what files were produced

Don't be interactive, don't wait for user input. Do the work and exit.
