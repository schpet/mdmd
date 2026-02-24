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
        mermaid.initialize({ startOnLoad: true });
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
