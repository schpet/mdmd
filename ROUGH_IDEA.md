# mdmd

mdmd is a markdown pager. it lets you skip to headings in the same way as delta (https://github.com/dandavison/delta - clone it into /tmp/delta if its not there already). n and p should go up and down headings. if you press a shortcut it should work like zed's cmd-shift-o shortcut to show you an outline of the markdown headings, using indentation and '#' symbols to denote hierarchy. when you open that modal, the current heading that your cursor is in should be selected. when you move up and down headings in that menu, the page behind it should jump up and down to different heading sections. there should be a slash based search, like vim. break this into tasks. manually qa each feature after you implement it, and use automated tests, making sure to run cargo test, cargo fmt and committing with jj after a feature is working.

mdmd also lets you follow links to other markdown documents. it takes inspiration from existing accessibility interfaces to highlight sections, move between headings, and move between links.

the question mark should bring up a shortcuts dialogue that is filterable, clearly lists shortcuts.

ensure each steps has a testing 

## Core Principle: Test as You Build

**DO NOT** write large chunks of code and then test. Instead:

1. Implement ONE small feature
2. Build and run the app
3. Manually verify it works
4. Document what you verified
5. Commit the working feature
6. Move to next feature


## tech stack

- rust/cargo
- frankentui see /home/exedev/repos/frankentui/docs/getting-started.md /home/exedev/repos/frankentui/docs/tutorials/agent-harness.md
