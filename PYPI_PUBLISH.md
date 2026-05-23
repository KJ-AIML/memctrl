# PyPI Publish Guide — MemCtrl

> This package is ready for PyPI. Follow these steps to publish.

---

## Prerequisites

- PyPI account: https://pypi.org/account/register/
- GitHub repo: https://github.com/KJ-AIML/memctrl

---

## Step 1: Verify the Build

```bash
python -m build
```

You should see:
```
Successfully built memctrl-1.0.0.tar.gz and memctrl-1.0.0-py3-none-any.whl
```

✅ Already verified — builds cleanly.

---

## Step 2: Configure Trusted Publishing on PyPI

Trusted Publishing uses OpenID Connect (OIDC) — no API tokens needed.

1. Go to https://pypi.org/manage/account/publishing/
2. Click **"Add a new pending publisher"**
3. Fill in:
   - **PyPI Project Name**: `memctrl`
   - **Owner**: `KJ-AIML`
   - **Repository name**: `memctrl`
   - **Workflow name**: `publish.yml`
   - **Environment name**: `pypi`
4. Click **"Add"**

---

## Step 3: Publish via GitHub Release

1. Go to https://github.com/KJ-AIML/memctrl/releases
2. Click **"Draft a new release"**
3. Choose tag: `v1.0.0` (create new tag)
4. Release title: `MemCtrl v1.0.0 — Cognitive Memory Runtime`
5. Description: copy from `DISTRIBUTION.md` → Release Notes section
6. Click **"Publish release"**

The GitHub Action `.github/workflows/publish.yml` will auto-trigger and publish to PyPI.

---

## Step 4: Verify

Wait 2-3 minutes, then check:

```bash
pip install memctrl
memctrl --version
```

Should output: `MemCtrl v1.0.0`

---

## Step 5: Update README Badge

After publish, the PyPI badge in README will auto-update:

```markdown
[![PyPI](https://img.shields.io/pypi/v/memctrl.svg)](https://pypi.org/project/memctrl/)
```

Replace the current placeholder badge with this dynamic one.

---

## Troubleshooting

### "Trusted publisher verification failed"
- Double-check the workflow name is exactly `publish.yml`
- Ensure the environment name in PyPI matches `pypi` (in the workflow)

### "Project name already taken"
- Check if `memctrl` exists: https://pypi.org/project/memctrl/
- If taken, consider `memctrl-ai` or `agent-memory`
- Update `pyproject.toml` name field

### Build fails
- Ensure `hatchling` is installed: `pip install hatchling`
- Ensure version in `memctrl/__init__.py` matches `pyproject.toml`

---

## Post-Publish Checklist

- [ ] `pip install memctrl` works on clean machine
- [ ] `memctrl --version` shows correct version
- [ ] PyPI page renders README correctly
- [ ] Badges update on GitHub
- [ ] Announce on X/Twitter, HN, Reddit
