# Domain docs

How the engineering skills read this repo's domain documentation. This repo is single-context.

## Before exploring, read these

- **`CONTEXT.md`** at the repo root.
- **`docs/adr/`**. Read the ADRs that touch the area you will work in.

If these files do not exist, **proceed silently**. Do not flag their absence and do not suggest creating them upfront. The producer skill (`/grill-with-docs`) creates them when terms or decisions get resolved.

## File structure

```
/
├── CONTEXT.md
├── docs/adr/
│   └── 0001-<decision>.md
└── src/
```

## Use the glossary's vocabulary

When your output names a domain concept (in an issue title, a refactor proposal, a hypothesis or a test name), use the term as `CONTEXT.md` defines it. Do not drift to synonyms that the glossary avoids.

If the concept is not in the glossary yet, either you are inventing language the project does not use, or there is a real gap. Reconsider in the first case. Note it for `/grill-with-docs` in the second.

## Flag ADR conflicts

If your output contradicts an existing ADR, say so explicitly instead of silently overriding it.

> _Contradicts ADR-0001 (...), but worth reopening because..._
