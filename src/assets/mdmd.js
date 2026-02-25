/* mdmd.js — TOC active-heading highlight, Mermaid initialisation, theme toggle, and indentation hierarchy toggle */
(function () {
    'use strict';

    /* --------------------------------------------------------------------- *
     * Mermaid initialisation stub                                            *
     * The CDN <script> tag is injected by bd-2se; this stub calls           *
     * mermaid.initialize once the DOM and (potentially) the CDN script are  *
     * ready.  When the CDN script is absent the guard keeps this a no-op.   *
     * --------------------------------------------------------------------- */
    if (typeof mermaid !== 'undefined') {
        var isDark = document.documentElement.getAttribute('data-theme') === 'dark';
        mermaid.initialize({ startOnLoad: true, theme: isDark ? 'dark' : 'default' });
    }

    /* --------------------------------------------------------------------- *
     * TOC active-heading highlight via IntersectionObserver                 *
     * --------------------------------------------------------------------- */

    var headingEls = Array.from(
        document.querySelectorAll(
            'main.content h1, main.content h2, main.content h3,' +
            'main.content h4, main.content h5, main.content h6'
        )
    );

    /* No headings: skip TOC observer setup entirely.  The indentation-hierarchy
     * toggle runs in its own IIFE below and is unaffected by this return. */
    if (headingEls.length === 0) { return; }

    /* Track which heading IDs are currently intersecting (in the top 20% of
     * the viewport).  The topmost one in document order becomes "active". */
    var visibleIds = new Set();

    function updateActive() {
        var activeId = null;
        for (var i = 0; i < headingEls.length; i++) {
            if (visibleIds.has(headingEls[i].id)) {
                activeId = headingEls[i].id;
                break;
            }
        }

        document.querySelectorAll('.toc-sidebar a').forEach(function (a) {
            var href = a.getAttribute('href') || '';
            var id = href.charAt(0) === '#' ? href.slice(1) : null;
            if (id !== null && id === activeId) {
                a.classList.add('active');
            } else {
                a.classList.remove('active');
            }
        });
    }

    var observer = new IntersectionObserver(function (entries) {
        entries.forEach(function (entry) {
            if (entry.isIntersecting) {
                visibleIds.add(entry.target.id);
            } else {
                visibleIds.delete(entry.target.id);
            }
        });
        updateActive();
    }, {
        rootMargin: '0px 0px -80% 0px'
    });

    headingEls.forEach(function (el) {
        observer.observe(el);
    });

    /* --- bd-1zl.5.1: rebindHeadingObserver --------------------------------- *
     *                                                                           *
     * Rebuilds observer bindings after DOM restructuring on mode transitions.  *
     * Called by unwrapOutlineSections (OFF path) via                           *
     * window.mdmd.rebindHeadingObserver.                                       *
     *                                                                           *
     *   1. Disconnect existing observer so stale observations are cleared.    *
     *   2. Clear visibleIds — stale IDs must not survive across resets.        *
     *   3. Re-query fresh heading node references from the live DOM.          *
     *   4. Re-observe each heading with the (now disconnected) observer.      *
     *   5. Call updateActive() to clear any stale .active state.             *
     *                                                                           *
     * On no-heading documents, re-query returns []; disconnect() is safe and  *
     * updateActive() is a no-op.                                               *
     * ----------------------------------------------------------------------- */
    function rebindHeadingObserver() {
        if (observer) { observer.disconnect(); }
        visibleIds.clear();
        headingEls = Array.from(
            document.querySelectorAll(
                'main.content h1, main.content h2, main.content h3,' +
                'main.content h4, main.content h5, main.content h6'
            )
        );
        headingEls.forEach(function (el) { observer.observe(el); });
        updateActive();
    }

    /* Expose for cross-IIFE use (bd-1zl.5.1). */
    window.mdmd = window.mdmd || {};
    window.mdmd.rebindHeadingObserver = rebindHeadingObserver;
}());

/* --------------------------------------------------------------------- *
 * Theme toggle button                                                    *
 * --------------------------------------------------------------------- */
(function () {
    var btn = document.getElementById('theme-toggle');
    if (!btn) { return; }
    btn.addEventListener('click', function () {
        var current = document.documentElement.getAttribute('data-theme');
        // Determine effective current theme (account for system default)
        var effectivelyDark = current === 'dark' ||
            (!current && window.matchMedia('(prefers-color-scheme: dark)').matches);
        var next = effectivelyDark ? 'light' : 'dark';
        document.documentElement.setAttribute('data-theme', next);
        try { localStorage.setItem('mdmd-theme', next); } catch (_) {}
    });
}());

/* --------------------------------------------------------------------- *
 * Shared namespace for cross-IIFE integration hooks                    *
 * rebindHeadingObserver is assigned by the TOC IIFE above (bd-1zl.5.1)*
 * --------------------------------------------------------------------- */
window.mdmd = window.mdmd || {};

/* --------------------------------------------------------------------- *
 * Indentation hierarchy toggle (bd-1zl)                                *
 *                                                                       *
 * Runs unconditionally — no early-return on heading count — so pages   *
 * with no headings still get a functional toggle and persisted state.   *
 *                                                                       *
 * Order of operations:                                                  *
 *   1. Read persistence from localStorage                               *
 *   2. Apply / remove root class on <html>  (idempotent with FOUC      *
 *      inline script that ran before first paint)                       *
 *   3. Apply DOM outline transform if active (bd-1zl.3)                *
 *   4. Bind button click handler                                        *
 *                                                                       *
 * State contract (matches INDENT_INIT_SCRIPT in html.rs):              *
 *   Storage key : mdmd-indent-hierarchy                                 *
 *   Legal values: 'on' | 'off'                                         *
 *   Root class  : indent-hierarchy-on  on  <html>                      *
 *   Default     : off  (class absent; malformed/missing values → off)  *
 * --------------------------------------------------------------------- */
(function () {
    'use strict';

    var INDENT_KEY   = 'mdmd-indent-hierarchy';
    var INDENT_ON    = 'on';
    var INDENT_OFF   = 'off';
    var INDENT_CLASS = 'indent-hierarchy-on';

    /* --- bd-1zl.3.1: Heading-stack traversal -------------------------------- *
     *                                                                          *
     * Scans direct children of mainEl in document order and produces a plan  *
     * describing which generated section wrappers to create and which DOM     *
     * nodes belong in each wrapper.  No DOM mutations are performed here.     *
     *                                                                          *
     * Returns [] when no headings are found (no-op guard).                   *
     * Otherwise returns an ordered array of plan entries:                     *
     *   { sectionEl, depth, parent, children }                               *
     *                                                                          *
     * Algorithm:                                                               *
     *   - Snapshot children via Array.from (flat NodeList from comrak).       *
     *   - Scan linearly; each heading (H1..H6) opens a new section group.    *
     *   - Level stack [{level, sectionEl}] tracks open sections:             *
     *       1. Pop entries where stack.top.level >= current heading level.    *
     *       2. Stack top is the new section's parent (mainEl if empty).       *
     *       3. Push {level, sectionEl: newWrapper}.                           *
     *   - depth = stack.length after popping, before pushing (1-based).      *
     *   - Pre-heading nodes (depth 0) remain in mainEl untouched.            *
     *   - Post-heading non-heading nodes are collected into the current       *
     *     topmost section's children list.                                    *
     * ----------------------------------------------------------------------- */
    function planOutlineSections(mainEl) {
        var children = Array.from(mainEl.children);

        /* Guard: return empty plan when the page has no headings. */
        var hasHeading = children.some(function (c) {
            var t = c.tagName;
            return t === 'H1' || t === 'H2' || t === 'H3' ||
                   t === 'H4' || t === 'H5' || t === 'H6';
        });
        if (!hasHeading) { return []; }

        var plan         = [];
        var stack        = []; /* [{level: number, sectionEl: Element}] */
        var currentEntry = null;

        for (var i = 0; i < children.length; i++) {
            var node = children[i];
            var tag  = node.tagName;
            var lvl  = tag === 'H1' ? 1 : tag === 'H2' ? 2 : tag === 'H3' ? 3 :
                       tag === 'H4' ? 4 : tag === 'H5' ? 5 : tag === 'H6' ? 6 : 0;

            if (lvl === 0) {
                /* Non-heading node. */
                if (currentEntry === null) { continue; } /* depth-0: leave in place */
                currentEntry.children.push(node);        /* collect into current section */
                continue;
            }

            /* Heading node — close sections at same or higher level. */
            while (stack.length > 0 && stack[stack.length - 1].level >= lvl) {
                stack.pop();
            }

            var depth  = stack.length + 1;
            var parent = stack.length > 0 ? stack[stack.length - 1].sectionEl : mainEl;

            /* Create wrapper element (not yet inserted into DOM). */
            var wrapper = document.createElement('section');
            wrapper.className = 'indent-section';
            wrapper.setAttribute('data-indent-generated', '1');
            wrapper.setAttribute('data-depth', String(depth));

            var entry = {
                sectionEl : wrapper,
                depth     : depth,
                parent    : parent,
                children  : [node] /* heading is always the first child */
            };
            plan.push(entry);
            currentEntry = entry;

            stack.push({ level: lvl, sectionEl: wrapper });
        }

        return plan;
    }

    /* --- bd-1zl.3.2: Materialization (consume plan, write DOM) ------------- *
     *                                                                          *
     * Inserts generated section wrappers and moves planned nodes into them.   *
     *                                                                          *
     *   - Top-level sections (parent === mainEl): inserted at the heading's   *
     *     current DOM position via insertBefore.                               *
     *   - Nested sections (parent is another sectionEl): appended to parent.  *
     *   - All children (heading first, then content) are moved into the       *
     *     wrapper via appendChild — no cloning, ids/attrs preserved.          *
     *   - Sets mainEl.dataset.indentActive = '1' on success.                 *
     * ----------------------------------------------------------------------- */
    function materializeOutlineSections(mainEl, plan) {
        if (plan.length === 0) { return; }

        plan.forEach(function (entry) {
            var sectionEl = entry.sectionEl;
            var parent    = entry.parent;
            var heading   = entry.children[0];

            /* Insert wrapper at heading's original position or append to parent. */
            if (parent === mainEl) {
                mainEl.insertBefore(sectionEl, heading);
            } else {
                parent.appendChild(sectionEl);
            }

            /* Move all planned nodes into wrapper (heading + content). */
            entry.children.forEach(function (child) {
                sectionEl.appendChild(child);
            });
        });

        /* Mark transform complete so idempotency guards can check this flag. */
        mainEl.dataset.indentActive = '1';
    }

    /* --- bd-1zl.3: DOM outline section builder ----------------------------- *
     *                                                                          *
     * Composes traversal and materialization:                                  *
     *   1. planOutlineSections        — compute section structure (bd-1zl.3.1)*
     *   2. materializeOutlineSections — write DOM changes       (bd-1zl.3.2) *
     * ----------------------------------------------------------------------- */
    function buildOutlineSections(mainEl) {
        var plan = planOutlineSections(mainEl);
        materializeOutlineSections(mainEl, plan);
    }

    /* --- bd-1zl.4.1: Canonical unwrap (OFF restore path) ------------------- *
     *                                                                          *
     * Removes all generated wrappers (data-indent-generated="1") from under   *
     * mainEl in reverse document order, moving their children back in place.  *
     * Only wrappers bearing the generated marker are touched; authored         *
     * <section> nodes are never removed.                                       *
     *                                                                          *
     * Algorithm:                                                               *
     *   1. Select wrappers in reverse document order (deepest-first).         *
     *   2. For each wrapper, move all child nodes before the wrapper.         *
     *   3. Remove the now-empty wrapper.                                       *
     *   4. Clear mainEl.dataset.indentActive.                                 *
     *                                                                          *
     * OFF guard: if the active marker is absent the transform was never        *
     * applied (or already removed) — return immediately as a no-op.           *
     * ----------------------------------------------------------------------- */
    function unwrapOutlineSections(mainEl) {
        if (!mainEl || !mainEl.dataset.indentActive) { return; }

        /* querySelectorAll returns document order; reverse for deepest-first
         * so inner wrappers are unwrapped before their parents. */
        var wrappers = Array.from(
            mainEl.querySelectorAll('[data-indent-generated="1"]')
        ).reverse();

        wrappers.forEach(function (wrapper) {
            /* Move all children before the wrapper, preserving document order. */
            while (wrapper.firstChild) {
                wrapper.parentNode.insertBefore(wrapper.firstChild, wrapper);
            }
            wrapper.parentNode.removeChild(wrapper);
        });

        delete mainEl.dataset.indentActive;

        /* Defensive observer rebind (bd-1zl.5.1) — no-op until that task
         * assigns window.mdmd.rebindHeadingObserver.  Heading element
         * references held by the TOC observer become stale after unwrap;
         * calling rebind ensures the observer tracks the live DOM nodes. */
        var ns = window.mdmd;
        if (ns && typeof ns.rebindHeadingObserver === 'function') {
            ns.rebindHeadingObserver();
        }
    }

    /* --- bd-1zl.4.1: Transition-aware OFF sequencing ----------------------- *
     *                                                                          *
     * 1. Remove root class first so CSS padding can animate to baseline.      *
     * 2. Persist OFF state immediately (aria/localStorage stays consistent     *
     *    even if the caller is a no-op).                                      *
     * 3. Prefers-reduced-motion: unwrap immediately without waiting.          *
     * 4. Otherwise: listen for transitionend on mainEl; a 350 ms timeout      *
     *    fallback ensures OFF never hangs when no transition fires.           *
     * ----------------------------------------------------------------------- */
    function applyIndentOff(mainEl) {
        /* Class removal first — CSS animation starts from this point. */
        document.documentElement.classList.remove(INDENT_CLASS);
        try { localStorage.setItem(INDENT_KEY, INDENT_OFF); } catch (_) {}

        /* OFF guard: nothing to unwrap if transform was never applied. */
        if (!mainEl || !mainEl.dataset.indentActive) { return; }

        var prefersReduced = window.matchMedia &&
            window.matchMedia('(prefers-reduced-motion: reduce)').matches;

        if (prefersReduced) {
            unwrapOutlineSections(mainEl);
            return;
        }

        /* Transition-aware path: unwrap after CSS completes.  The timeout
         * fires unconditionally so unwrap cannot be skipped when the element
         * carries no transition (e.g. before bd-1zl.6 CSS lands). */
        var done = false;
        function doUnwrap() {
            if (done) { return; }
            done = true;
            mainEl.removeEventListener('transitionend', doUnwrap);
            /* Idempotency guard (bd-1zl.4.2): if the state was toggled back
             * to ON before the transition completed, the wrappers are still
             * in the DOM and the class has been re-added — skip unwrap so
             * DOM and class stay in sync. */
            if (document.documentElement.classList.contains(INDENT_CLASS)) { return; }
            unwrapOutlineSections(mainEl);
        }
        setTimeout(doUnwrap, 350);
        mainEl.addEventListener('transitionend', doUnwrap, { once: true });
    }

    /* Read saved preference; normalize unknown/missing to off. */
    var saved;
    try { saved = localStorage.getItem(INDENT_KEY); } catch (_) { saved = null; }
    var active = saved === INDENT_ON;

    var mainEl = document.querySelector('main.content');

    /* Apply class (idempotent — FOUC script already ran). */
    if (active) {
        document.documentElement.classList.add(INDENT_CLASS);
    } else {
        document.documentElement.classList.remove(INDENT_CLASS);
    }

    /* Apply DOM outline transform on page load if the mode is already active. */
    if (active && mainEl && !mainEl.dataset.indentActive) {
        buildOutlineSections(mainEl);
    }

    /* Bind toggle button once it exists (added by bd-1zl.2). */
    var btn = document.getElementById('indent-toggle');
    if (!btn) { return; }

    btn.addEventListener('click', function () {
        active = !active;
        if (active) {
            document.documentElement.classList.add(INDENT_CLASS);
            try { localStorage.setItem(INDENT_KEY, INDENT_ON); } catch (_) {}
            /* ON guard (bd-1zl.4.2): skip transform if already applied. */
            if (mainEl && !mainEl.dataset.indentActive) {
                buildOutlineSections(mainEl);
            }
        } else {
            /* OFF path uses transition-aware sequencing with unwrap (bd-1zl.4.1).
             * OFF guard inside applyIndentOff handles the already-off case. */
            applyIndentOff(mainEl);
        }
    });
}());

/* --------------------------------------------------------------------- *
 * File-change detection: poll /_mdmd/freshness and reveal notice div   *
 * when the server-side mtime changes (bd-38z).                         *
 * --------------------------------------------------------------------- */
(function () {
    var meta_mtime = document.querySelector('meta[name="mdmd-mtime"]');
    var meta_path = document.querySelector('meta[name="mdmd-path"]');
    if (!meta_mtime || !meta_path) { return; }
    var initial_mtime = parseInt(meta_mtime.content, 10);
    var page_path = meta_path.content; // norm_display WITHOUT leading slash
    var failures = 0;
    var MAX_FAILURES = 3;
    var interval = setInterval(function () {
        fetch('/_mdmd/freshness?path=' + encodeURIComponent(page_path))
            .then(function (r) { return r.ok ? r.json() : Promise.reject('non-200'); })
            .then(function (data) {
                failures = 0;
                if (data.mtime !== initial_mtime) {
                    clearInterval(interval);
                    var notice = document.getElementById('mdmd-change-notice');
                    if (notice) { notice.removeAttribute('hidden'); }
                }
            })
            .catch(function () {
                failures++;
                if (failures >= MAX_FAILURES) { clearInterval(interval); }
            });
    }, 4000);
}());
