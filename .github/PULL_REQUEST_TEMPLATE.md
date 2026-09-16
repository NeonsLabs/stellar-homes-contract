## What this changes

<!-- What behaviour is different after this PR, and why. -->

## Why

<!-- The problem being solved. Link the issue if there is one. -->

## Risk

<!-- Which contracts does this touch, and what is the worst case if it is
     wrong? Call out anything that moves funds, changes authorization, or
     alters stored state layout. -->

- [ ] Touches a path that moves the settlement asset
- [ ] Changes who may call something
- [ ] Changes a stored type (needs a migration plan for deployed instances)
- [ ] Changes the wasm size baseline

## Checklist

- [ ] `make all` passes locally (fmt, clippy, tests, build)
- [ ] New behaviour is covered by tests, including the ways it should fail
- [ ] Public functions carry doc comments explaining *why*, not just what
- [ ] `docs/` updated if roles, storage or the deployment sequence changed
