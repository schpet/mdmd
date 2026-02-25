/* mdmd.js — TOC active-heading highlight and Mermaid initialisation stub */
(function () {
    'use strict';

    /* --------------------------------------------------------------------- *
     * Mermaid initialisation stub                                            *
     * The CDN <script> tag is injected by bd-2se; this stub calls           *
     * mermaid.initialize once the DOM and (potentially) the CDN script are  *
     * ready.  When the CDN script is absent the guard keeps this a no-op.   *
     * --------------------------------------------------------------------- */
    if (typeof mermaid !== 'undefined') {
        mermaid.initialize({ startOnLoad: true, theme: 'default' });
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
