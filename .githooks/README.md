# Git Hooks

This repository uses a local commit message hook that enforces the format:

```text
CC(scope): subject
```

Example:

```text
CC(readme): rewrite the English documentation
```

To enable the hook in this clone:

```sh
git config core.hooksPath .githooks
```
