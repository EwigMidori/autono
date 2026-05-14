# Git Hooks

This repository uses a local commit message hook that enforces the format:

```text
type(scope): subject
```

Example:

```text
feat(refaco): rewrite the English documentation
```

To enable the hook in this clone:

```sh
git config core.hooksPath .githooks
```
