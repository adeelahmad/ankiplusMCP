# User space

The loop's opinions, ejected from the aloop plugin 0.13.0 so this project can change them.
Change anything under `agents/` and `skills/`: it takes effect on the next session (or
`aloop userspace apply`). TDD is the default opinion, not a rule.

What you cannot change from here is the **kernel**: the gates, the audit log and the
hooks that run them. `reference/` holds a copy of them to read; editing it does nothing.
Add checks of your own as modules: `~/.aloop/modules/<name>/`, run once this project adds them to
`distro.json` (`aloop module add <name>`). They add checks; they never remove a gate. A distro bundles
a domain's modules, roles and skills (`aloop distro`; `software` is the default). See
docs/BUILD-A-MODULE.md in the aloop repo.

Layers, highest first: your home copy (`aloop userspace path --home`), this folder, the plugin.

    aloop userspace status          what is customized, stale or in conflict
    aloop userspace diff <rel>      your copy against the plugin's
    aloop userspace merge <rel>     three-way merge a plugin upgrade into your copy
    aloop userspace accept <rel>    keep your copy as resolved against the new plugin text
    aloop userspace reset <rel>     go back to the plugin's copy

`.base/` and `manifest.json` are what the upgrade compares with: commit them too.
