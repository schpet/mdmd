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

    if (headingEls.length === 0) {
        return;
    }

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

    /* --- bd-1zl.3: DOM outline section builder ----------------------------- *
     *                                                                         *
     * Traverses direct children of main.content in document order and wraps  *
     * heading-delimited groups in generated <section> elements:               *
     *                                                                         *
     *   <section class="indent-section"                                       *
     *            data-indent-generated="1"                                    *
     *            data-depth="N">                                              *
     *                                                                         *
     * Algorithm (bd-1zl.3.1 — heading-stack traversal):                      *
     *   - Snapshot children as an Array (flat NodeList from comrak output).   *
     *   - Scan linearly; each heading (H1..H6) opens a new section group.    *
     *   - Level stack [{level, sectionEl}] tracks open sections:             *
     *       1. Pop entries where stack.top.level >= current heading level.    *
     *       2. Stack top is the new section's parent (mainEl if empty).      *
     *       3. Push {level, sectionEl: newWrapper}.                          *
     *   - depth = stack.length after popping, before pushing (1-based).      *
     *   - Pre-heading nodes (depth 0) remain in mainEl untouched.            *
     *   - Post-heading non-heading nodes move into the topmost section.      *
     *   - Guard: if no headings exist, return immediately (no-op).           *
     *                                                                         *
     * Materialization (bd-1zl.3.2):                                          *
     *   - Wrapper inserted at heading's current position when parent=mainEl. *
     *   - Wrapper appended to parent section when nesting.                   *
     *   - Heading node moved into wrapper (never cloned, id/attrs preserved). *
     *   - Sets mainEl.dataset.indentActive = '1' on success.                *
     * ----------------------------------------------------------------------- */
    function buildOutlineSections(mainEl) {
        var children = Array.from(mainEl.children);

        /* Guard: no-op when the page has no headings. */
        var hasHeading = children.some(function (c) {
            var t = c.tagName;
            return t === 'H1' || t === 'H2' || t === 'H3' ||
                   t === 'H4' || t === 'H5' || t === 'H6';
        });
        if (!hasHeading) { return; }

        var stack = []; /* [{level: number, sectionEl: Element}] */
        var firstHeadingFound = false;

        for (var i = 0; i < children.length; i++) {
            var node = children[i];
            var tag  = node.tagName;
            var lvl  = tag === 'H1' ? 1 : tag === 'H2' ? 2 : tag === 'H3' ? 3 :
                       tag === 'H4' ? 4 : tag === 'H5' ? 5 : tag === 'H6' ? 6 : 0;

            if (lvl === 0) {
                /* Non-heading node. */
                if (!firstHeadingFound) { continue; } /* depth-0: leave in place */
                /* Move into the current topmost section. */
                stack[stack.length - 1].sectionEl.appendChild(node);
                continue;
            }

            /* Heading node — open a new section. */
            firstHeadingFound = true;

            /* Close sections at same or higher level (bd-1zl.3.1 step 1). */
            while (stack.length > 0 && stack[stack.length - 1].level >= lvl) {
                stack.pop();
            }

            var depth  = stack.length + 1;
            var parent = stack.length > 0 ? stack[stack.length - 1].sectionEl : mainEl;

            /* Create wrapper (bd-1zl.3.2 contract). */
            var wrapper = document.createElement('section');
            wrapper.className = 'indent-section';
            wrapper.setAttribute('data-indent-generated', '1');
            wrapper.setAttribute('data-depth', String(depth));

            /* Insert at heading's current DOM position (mainEl parent) or
             * append to parent section (nested case). */
            if (parent === mainEl) {
                mainEl.insertBefore(wrapper, node);
            } else {
                parent.appendChild(wrapper);
            }

            /* Move heading into wrapper (no clone, preserves id/class/data). */
            wrapper.appendChild(node);

            stack.push({ level: lvl, sectionEl: wrapper });
        }

        /* Mark transform complete so idempotency guards can check this flag. */
        mainEl.dataset.indentActive = '1';
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
            if (mainEl && !mainEl.dataset.indentActive) {
                buildOutlineSections(mainEl);
            }
        } else {
            document.documentElement.classList.remove(INDENT_CLASS);
            try { localStorage.setItem(INDENT_KEY, INDENT_OFF); } catch (_) {}
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
