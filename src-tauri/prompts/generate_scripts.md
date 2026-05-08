You are auto-generating convenience launcher scripts in this project's `.ccpscript/` directory.

## Task

1. Read the project's key files (`package.json` / `Cargo.toml` / `pyproject.toml` / `docker-compose.yml` / `Makefile` / `Procfile` / `*.csproj` etc.) to identify the tech stack and supported commands.

2. Create corresponding PowerShell scripts (`.ps1`) under `.ccpscript/`. Common tasks:
   - Start development server (dev / start / serve)
   - Build (build)
   - Test (test)
   - Lint / typecheck

   Only generate scripts that **actually exist and make sense** for this project. Less is more.

3. Each script's first line MUST be `# Title: <concise title>`, e.g.:
   ```ps1
   # Title: Start dev server
   npm run dev
   ```

4. Keep script content concise. Common commands (like `npm run dev`) on one line; multi-step flows (cd into subdir, switch Node version) on multiple lines.

## Critical: lightweight static checks

After each script is generated, run these three checks. **Do NOT actually run business commands** (no starting dev servers, no real build/test, no `npm install`).

### Step 1: PowerShell syntax parse (no execution)

```powershell
powershell -NoProfile -Command "$null = [ScriptBlock]::Create([System.IO.File]::ReadAllText('.ccpscript/<name>.ps1'))"
```

Exit code 0 = syntax valid; non-zero = the script has a typo. **Fix the script, retry at most once.**

### Step 2: Command reference verification

For each command in the script, verify references exist (without executing the command):

- Contains `npm run X` / `pnpm X` / `yarn X` / `bun X` → read `package.json`, verify `scripts.X` actually exists
- Contains `cargo X` → verify `Cargo.toml` exists
- Contains `docker compose ...` or `docker-compose ...` → verify `docker-compose.yml` / `compose.yaml` exists
- Contains `python X.py` or `python -m Y` → verify `X.py` or module `Y` exists
- Contains `go run` / `go build` → verify `go.mod` exists
- Contains `Set-Location X` / `cd X` (relative path) → verify the subdirectory exists
- Explicitly references `.env` or config files → verify they exist

Any hard error (e.g. script calls `npm run dev` but `package.json` has no `dev` script) → **fix the script, retry at most once**.

### Step 3: Tool PATH check

For each main external command in the script (`npm` / `cargo` / `docker` / `python` etc.):

```powershell
Get-Command <tool> -ErrorAction SilentlyContinue
```

If empty → tool not on PATH. **Do NOT delete the script** (the user may switch environments / version managers), but add a comment near the top, for example:

```ps1
# Title: Start dev server
# Note: 'pnpm' was not on PATH at generation time — make sure your environment provides it.
pnpm dev
```

## Don't do

- ❌ **Don't actually start a dev server** (don't `run_in_background` something like `npm run dev`)
- ❌ **Don't actually run build / test** (may produce build artifacts, modify lockfiles)
- ❌ **Don't run `npm install` / `pnpm install` / `cargo build`**
- ❌ **Don't spend more than 5 seconds verifying a single script**

## Key principles

- Checks only catch "obvious mistakes": typos, references to nonexistent npm scripts, missing tools
- Real runtime issues (port conflicts, missing env vars) are left for the user to discover when they click ▶
- If verification fails and one retry doesn't fix it → **delete the script**
- When done, briefly list: ✓ passed, ⚠ kept with a note, ✗ deleted (with reason)

Don't be interactive, don't ask questions, don't wait for user input. Do the work and exit.
