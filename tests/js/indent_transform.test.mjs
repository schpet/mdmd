/**
 * tests/js/indent_transform.test.mjs
 *
 * Unit tests for the indentation-hierarchy toggle in src/assets/mdmd.js.
 *
 * Coverage (per bd-1zl.7.2):
 *   1.  ON->OFF->ON wrapper count invariant
 *   2.  ON->ON no additional wrappers (idempotency guard)
 *   3.  OFF->OFF no throw and no active marker
 *   4.  OFF clears generated markers and wrapper nodes
 *   5.  Stored 'on' applies root class at init
 *   6.  Stored 'off' / missing / invalid values normalize to OFF
 *   7.  localStorage access failure (throws) does not break init
 *   8.  No-heading document still initializes button (no crash)
 *
 * Run:  node --test tests/js/indent_transform.test.mjs
 */

import { describe, test } from 'node:test';
import assert             from 'node:assert/strict';
import vm                 from 'node:vm';
import { readFileSync }   from 'node:fs';
import { fileURLToPath }  from 'node:url';
import path               from 'node:path';

// ---------------------------------------------------------------------------
// Source extraction
// ---------------------------------------------------------------------------
const __dirname  = path.dirname(fileURLToPath(import.meta.url));
const MDMD_SRC   = readFileSync(
    path.join(__dirname, '../../src/assets/mdmd.js'),
    'utf8'
);

/**
 * Locate and return only the indentation-hierarchy IIFE (the third IIFE in
 * mdmd.js).  Isolating it avoids needing a full browser-API surface for the
 * TOC and theme-toggle IIFEs.
 */
function extractIndentIIFE(src) {
    // The indent IIFE is uniquely identified by its opening lines.
    const MARKER = "(function () {\n    'use strict';\n\n    var INDENT_KEY";
    const idx    = src.indexOf(MARKER);
    if (idx === -1) {
        throw new Error(
            'Cannot locate indent-hierarchy IIFE in src/assets/mdmd.js — ' +
            'check that the marker string matches the source.'
        );
    }
    return src.slice(idx);
}

const INDENT_SRC = extractIndentIIFE(MDMD_SRC);

// ---------------------------------------------------------------------------
// Minimal DOM implementation
//
// Implements just enough of the browser DOM for the indent IIFE:
//   - Element  (tagName, dataset, classList, appendChild, insertBefore,
//               removeChild, querySelector, querySelectorAll, addEventListener)
//   - Document (documentElement, createElement, querySelector,
//               querySelectorAll, getElementById)
//   - localStorage  (getItem, setItem)
//   - window.matchMedia (with prefers-reduced-motion mock)
// ---------------------------------------------------------------------------

class DOMTokenList {
    constructor() { this._set = new Set(); }
    add(...tokens)      { tokens.forEach(t => this._set.add(t)); }
    remove(...tokens)   { tokens.forEach(t => this._set.delete(t)); }
    contains(token)     { return this._set.has(token); }
}

class FakeElement {
    constructor(tagName) {
        this.tagName   = tagName.toUpperCase();
        this.id        = '';
        this.className = '';
        this.classList = new DOMTokenList();
        // dataset backed by a plain object so `delete el.dataset.foo` works.
        this.dataset   = Object.create(null);
        this._attrs    = Object.create(null);
        this._children = [];
        this._parent   = null;
        this._listeners = Object.create(null); // type -> [{handler, once}]
    }

    // --- Child accessors ---
    get children()   { return [...this._children]; }
    get childNodes() { return [...this._children]; }
    get firstChild() { return this._children[0] ?? null; }
    get parentNode() { return this._parent; }

    // --- Attribute access ---
    getAttribute(name) {
        return this._attrs[name] !== undefined ? this._attrs[name] : null;
    }
    setAttribute(name, value) {
        this._attrs[name] = String(value);
        // Keep dataset in sync for data-* attributes.
        if (name.startsWith('data-')) {
            const key = name.slice(5).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
            this.dataset[key] = String(value);
        }
    }

    // --- Tree mutation ---
    _detach() {
        if (this._parent) {
            const idx = this._parent._children.indexOf(this);
            if (idx !== -1) this._parent._children.splice(idx, 1);
            this._parent = null;
        }
    }
    appendChild(child) {
        child._detach();
        this._children.push(child);
        child._parent = this;
        return child;
    }
    insertBefore(newNode, refNode) {
        newNode._detach();
        const idx = this._children.indexOf(refNode);
        if (idx === -1) return this.appendChild(newNode);
        this._children.splice(idx, 0, newNode);
        newNode._parent = this;
        return newNode;
    }
    removeChild(child) {
        const idx = this._children.indexOf(child);
        if (idx !== -1) {
            this._children.splice(idx, 1);
            child._parent = null;
        }
        return child;
    }

    // --- Events ---
    addEventListener(type, handler, options) {
        if (!this._listeners[type]) this._listeners[type] = [];
        const once = (options && options.once) === true;
        this._listeners[type].push({ handler, once });
    }
    removeEventListener(type, handler) {
        if (!this._listeners[type]) return;
        this._listeners[type] = this._listeners[type].filter(
            l => l.handler !== handler
        );
    }
    dispatchEvent(evt) {
        const listeners = (this._listeners[evt.type] || []).slice();
        for (const l of listeners) {
            if (l.once) this.removeEventListener(evt.type, l.handler);
            l.handler(evt);
        }
    }
    click() { this.dispatchEvent({ type: 'click' }); }

    // --- Selector engine (handles the subset used by the indent IIFE) ---
    _allDescendants() {
        const out = [];
        const visit = el => {
            el._children.forEach(c => { out.push(c); visit(c); });
        };
        visit(this);
        return out;
    }
    _matches(sel) {
        // [attr="val"]
        const attrM = sel.match(/^\[([^\]="]+)="([^"]*)"\]$/);
        if (attrM) return this.getAttribute(attrM[1]) === attrM[2];
        // tag.class
        const tcM = sel.match(/^([a-zA-Z]+)\.([a-zA-Z0-9_-]+)$/);
        if (tcM) {
            return this.tagName === tcM[1].toUpperCase() &&
                   this.classList.contains(tcM[2]);
        }
        // #id
        const idM = sel.match(/^#(.+)$/);
        if (idM) return this.id === idM[1];
        // bare tag
        if (/^[a-zA-Z]+$/.test(sel)) return this.tagName === sel.toUpperCase();
        return false;
    }
    querySelector(sel) {
        return this._allDescendants().find(e => e._matches(sel)) ?? null;
    }
    querySelectorAll(sel) {
        return this._allDescendants().filter(e => e._matches(sel));
    }
}

class FakeDocument {
    constructor() {
        this.documentElement = new FakeElement('html');
        // body is appended so _allDescendants traversal includes it first.
        const body = new FakeElement('body');
        this.documentElement.appendChild(body);
    }
    createElement(tag) { return new FakeElement(tag); }
    getElementById(id) {
        return this.documentElement._allDescendants().find(e => e.id === id) ?? null;
    }
    querySelector(sel) {
        return this.documentElement._allDescendants().find(e => e._matches(sel)) ?? null;
    }
    querySelectorAll(sel) {
        return this.documentElement._allDescendants().filter(e => e._matches(sel));
    }
}

class FakeLocalStorage {
    constructor(initial = {}) { this._store = { ...initial }; }
    getItem(key)        { return Object.prototype.hasOwnProperty.call(this._store, key) ? this._store[key] : null; }
    setItem(key, value) { this._store[key] = String(value); }
    removeItem(key)     { delete this._store[key]; }
}

// ---------------------------------------------------------------------------
// makeContext
//
// Builds a fresh vm context, optionally populates localStorage and the DOM,
// then runs the indent-hierarchy IIFE.  Returns the live context plus helpers.
//
// Options
//   storedValue          string | null | 'throw'
//                        Initial mdmd-indent-hierarchy value.  'throw' makes
//                        every localStorage access throw.
//   hasHeadings          bool  — whether to add H1/H2/H3 + <p> siblings under main.content
//   hasButton            bool  — whether to add #indent-toggle button
//   prefersReducedMotion bool  — mock matchMedia so applyIndentOff is synchronous
// ---------------------------------------------------------------------------
function makeContext({
    storedValue          = null,
    hasHeadings          = true,
    hasButton            = true,
    prefersReducedMotion = true,
} = {}) {
    const doc    = new FakeDocument();
    const htmlEl = doc.documentElement;

    // Build <main class="content"> with optional headings.
    const mainEl = doc.createElement('main');
    mainEl.className = 'content';
    mainEl.classList.add('content');
    htmlEl.appendChild(mainEl);

    if (hasHeadings) {
        // Three heading levels with a paragraph after each, matching a typical page.
        const addEl = (tag, id) => {
            const el = doc.createElement(tag);
            el.id = id || '';
            mainEl.appendChild(el);
            return el;
        };
        addEl('h1', 'section-a');
        addEl('p');
        addEl('h2', 'section-b');
        addEl('p');
        addEl('h3', 'section-c');
        addEl('p');
    } else {
        mainEl.appendChild(doc.createElement('p'));
    }

    // Optional toggle button.
    let btn = null;
    if (hasButton) {
        btn = doc.createElement('button');
        btn.id = 'indent-toggle';
        htmlEl.appendChild(btn);
    }

    // localStorage: may throw, be pre-populated, or be empty.
    let ls;
    if (storedValue === 'throw') {
        ls = {
            getItem()  { throw new Error('localStorage: access denied (test stub)'); },
            setItem()  { throw new Error('localStorage: access denied (test stub)'); },
        };
    } else {
        const init = (storedValue !== null)
            ? { 'mdmd-indent-hierarchy': storedValue }
            : {};
        ls = new FakeLocalStorage(init);
    }

    // matchMedia: returns `matches: true` for prefers-reduced-motion queries so
    // that applyIndentOff unwraps synchronously (no real transitionend needed).
    const matchMedia = query => ({
        matches: prefersReducedMotion && query.includes('prefers-reduced-motion'),
    });

    // Build and run the vm context.
    const ctx = vm.createContext({
        window       : { mdmd: {} },
        document     : doc,
        localStorage : ls,
        // Execute setTimeout callbacks immediately (reduced-motion path uses
        // it as a fallback, but with the mock above it is not reached).
        setTimeout   : fn => fn(),
        clearTimeout : () => {},
        // file-change detection IIFE tail — safe to no-op.
        setInterval  : () => 0,
        clearInterval: () => {},
    });
    ctx.window.matchMedia = matchMedia;

    vm.runInContext(INDENT_SRC, ctx);

    // Attach helpers for assertions.
    ctx._mainEl = mainEl;
    ctx._htmlEl = htmlEl;
    ctx._btn    = btn;
    ctx._ls     = ls;
    return ctx;
}

// ---------------------------------------------------------------------------
// Assertion helpers
// ---------------------------------------------------------------------------

/** Number of generated wrapper sections currently inside mainEl. */
function wrapperCount(mainEl) {
    return mainEl.querySelectorAll('[data-indent-generated="1"]').length;
}

/** Simulate a button click on the indent toggle. */
function clickToggle(ctx) {
    if (!ctx._btn) throw new Error('makeContext: hasButton=false — no button to click');
    ctx._btn.click();
}

// ---------------------------------------------------------------------------
// Tests — cycle / idempotency
// ---------------------------------------------------------------------------
describe('indent-hierarchy toggle — cycle/idempotency', () => {

    test('1. ON->OFF->ON wrapper count invariant', (t) => {
        const ctx    = makeContext({ storedValue: 'off', hasHeadings: true });
        const mainEl = ctx._mainEl;
        const htmlEl = ctx._htmlEl;

        const CASE  = 'ON->OFF->ON';
        const INIT  = 'off';

        // Initial state: OFF.
        assert.equal(wrapperCount(mainEl), 0,
            `[${CASE}] init=${INIT}: no wrappers before first click`);
        assert.ok(!mainEl.dataset.indentActive,
            `[${CASE}] init=${INIT}: indentActive absent before first click`);

        // Click 1: OFF → ON.
        clickToggle(ctx);
        const wrappersFirst = wrapperCount(mainEl);
        assert.ok(wrappersFirst > 0,
            `[${CASE}] init=${INIT}: wrappers created after first ON click (got ${wrappersFirst})`);
        assert.equal(mainEl.dataset.indentActive, '1',
            `[${CASE}] init=${INIT}: indentActive='1' set after first ON`);
        assert.ok(htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class present after first ON`);

        // Click 2: ON → OFF.
        clickToggle(ctx);
        assert.equal(wrapperCount(mainEl), 0,
            `[${CASE}] init=${INIT}: wrappers=0 after OFF click`);
        assert.ok(!mainEl.dataset.indentActive,
            `[${CASE}] init=${INIT}: indentActive cleared after OFF`);
        assert.ok(!htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class absent after OFF`);

        // Click 3: OFF → ON again.
        clickToggle(ctx);
        const wrappersReon = wrapperCount(mainEl);
        assert.equal(wrappersReon, wrappersFirst,
            `[${CASE}] init=${INIT}: wrapper count after re-ON (${wrappersReon}) ` +
            `equals first-ON count (${wrappersFirst})`);
        assert.equal(mainEl.dataset.indentActive, '1',
            `[${CASE}] init=${INIT}: indentActive='1' set after re-ON`);
        assert.ok(htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class present after re-ON`);
    });

    test('2. ON->ON no additional wrappers (idempotency guard)', (t) => {
        // Start with stored='on': init builds the wrappers.  Then OFF->ON
        // exercises the guard path where indentActive is already present on
        // entry to the ON branch.
        const ctx    = makeContext({ storedValue: 'on', hasHeadings: true });
        const mainEl = ctx._mainEl;

        const CASE = 'ON->ON (via init+cycle)';
        const INIT = 'on';

        const wrappersInit = wrapperCount(mainEl);
        assert.ok(wrappersInit > 0,
            `[${CASE}] init=${INIT}: init built ${wrappersInit} wrapper(s)`);
        assert.equal(mainEl.dataset.indentActive, '1',
            `[${CASE}] init=${INIT}: indentActive='1' set by init`);

        // OFF: wrappers removed, marker cleared.
        clickToggle(ctx);
        assert.equal(wrapperCount(mainEl), 0,
            `[${CASE}] init=${INIT}: wrappers=0 after OFF`);
        assert.ok(!mainEl.dataset.indentActive,
            `[${CASE}] init=${INIT}: indentActive absent after OFF`);

        // ON: guard must not double-wrap.
        clickToggle(ctx);
        const wrappersRebuilt = wrapperCount(mainEl);
        assert.equal(wrappersRebuilt, wrappersInit,
            `[${CASE}] init=${INIT}: re-ON wrapper count (${wrappersRebuilt}) ` +
            `equals init count (${wrappersInit}) — no double-wrapping`);
        assert.equal(mainEl.dataset.indentActive, '1',
            `[${CASE}] init=${INIT}: indentActive='1' set exactly once after re-ON`);
    });

    test('3. OFF->OFF no throw and no active marker', (t) => {
        const ctx    = makeContext({ storedValue: 'off', hasHeadings: true });
        const mainEl = ctx._mainEl;
        const htmlEl = ctx._htmlEl;

        const CASE = 'OFF->OFF';
        const INIT = 'off';

        // Verify initial OFF state.
        assert.ok(!mainEl.dataset.indentActive,
            `[${CASE}] init=${INIT}: indentActive absent at start`);
        assert.ok(!htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class absent at start`);

        // Go ON then back OFF.
        clickToggle(ctx);  // → ON
        clickToggle(ctx);  // → OFF (first)

        // Now firmly in OFF state — no marker, no class, no wrappers.
        assert.ok(!mainEl.dataset.indentActive,
            `[${CASE}] init=${INIT}: indentActive absent after ON->OFF`);
        assert.equal(wrapperCount(mainEl), 0,
            `[${CASE}] init=${INIT}: wrappers=0 after ON->OFF`);
        assert.ok(!htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class absent after ON->OFF`);

        // A second OFF click (OFF → ON) then immediately back (ON → OFF) should
        // also leave the state clean without throwing.
        assert.doesNotThrow(() => {
            clickToggle(ctx);  // ON
            clickToggle(ctx);  // OFF
        }, `[${CASE}] init=${INIT}: repeated OFF transitions must not throw`);

        assert.ok(!mainEl.dataset.indentActive,
            `[${CASE}] init=${INIT}: indentActive absent after second OFF cycle`);
        assert.equal(wrapperCount(mainEl), 0,
            `[${CASE}] init=${INIT}: wrappers=0 after second OFF cycle`);
    });

    test('4. OFF clears generated markers and wrapper nodes', (t) => {
        const ctx    = makeContext({ storedValue: 'off', hasHeadings: true });
        const mainEl = ctx._mainEl;
        const htmlEl = ctx._htmlEl;

        const CASE = 'OFF-clears';
        const INIT = 'off';

        // Turn ON.
        clickToggle(ctx);
        const wrappersOn = wrapperCount(mainEl);
        assert.ok(wrappersOn > 0,
            `[${CASE}] init=${INIT}: wrappers created on ON (got ${wrappersOn})`);
        assert.equal(mainEl.dataset.indentActive, '1',
            `[${CASE}] init=${INIT}: indentActive='1' on ON`);
        assert.ok(htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class present on ON`);

        // Turn OFF.
        clickToggle(ctx);

        // All generated <section data-indent-generated="1"> elements must be gone.
        assert.equal(wrapperCount(mainEl), 0,
            `[${CASE}] init=${INIT}: wrappers=0 after OFF (was ${wrappersOn})`);

        // Active marker must be deleted.
        assert.ok(!mainEl.dataset.indentActive,
            `[${CASE}] init=${INIT}: indentActive marker deleted after OFF`);

        // Root class must be removed.
        assert.ok(!htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class removed after OFF`);
    });

});

// ---------------------------------------------------------------------------
// Tests — init-path persistence
// ---------------------------------------------------------------------------
describe('indent-hierarchy toggle — init-path persistence', () => {

    test("5. Stored 'on' applies root class and DOM transform at init", (t) => {
        const ctx    = makeContext({ storedValue: 'on', hasHeadings: true });
        const mainEl = ctx._mainEl;
        const htmlEl = ctx._htmlEl;

        const CASE = 'init-on';
        const INIT = 'on';

        assert.ok(htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class 'indent-hierarchy-on' present on <html>`);
        assert.equal(mainEl.dataset.indentActive, '1',
            `[${CASE}] init=${INIT}: indentActive='1' (DOM transform applied)`);
        assert.ok(wrapperCount(mainEl) > 0,
            `[${CASE}] init=${INIT}: wrapper nodes created (wrapperCount=${wrapperCount(mainEl)})`);
    });

    test("6a. Stored 'off' normalizes to OFF — class absent, no wrappers", (t) => {
        const ctx    = makeContext({ storedValue: 'off', hasHeadings: true });
        const mainEl = ctx._mainEl;
        const htmlEl = ctx._htmlEl;

        const CASE = 'init-off';
        const INIT = 'off';

        assert.ok(!htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class absent`);
        assert.ok(!mainEl.dataset.indentActive,
            `[${CASE}] init=${INIT}: indentActive absent`);
        assert.equal(wrapperCount(mainEl), 0,
            `[${CASE}] init=${INIT}: wrappers=0`);
    });

    test('6b. Missing storage value normalizes to OFF', (t) => {
        const ctx    = makeContext({ storedValue: null, hasHeadings: true });
        const htmlEl = ctx._htmlEl;

        const CASE = 'init-missing';
        const INIT = 'null (missing)';

        assert.ok(!htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class absent`);
        assert.ok(!ctx._mainEl.dataset.indentActive,
            `[${CASE}] init=${INIT}: indentActive absent`);
        assert.equal(wrapperCount(ctx._mainEl), 0,
            `[${CASE}] init=${INIT}: wrappers=0`);
    });

    test("6c. Invalid storage value normalizes to OFF", (t) => {
        // 'enabled', 'true', '1', etc. are not the canonical 'on' value.
        for (const badValue of ['enabled', 'true', '1', 'yes', 'ON']) {
            const ctx    = makeContext({ storedValue: badValue, hasHeadings: true });
            const htmlEl = ctx._htmlEl;

            const CASE = 'init-invalid';
            const INIT = JSON.stringify(badValue);

            assert.ok(!htmlEl.classList.contains('indent-hierarchy-on'),
                `[${CASE}] init=${INIT}: root class absent for non-canonical value`);
            assert.equal(wrapperCount(ctx._mainEl), 0,
                `[${CASE}] init=${INIT}: wrappers=0 for non-canonical value`);
        }
    });

    test('7. localStorage access failure does not break init', (t) => {
        // storedValue='throw' makes every localStorage call throw.
        // The IIFE must catch and default to OFF without propagating the error.
        assert.doesNotThrow(() => {
            const ctx    = makeContext({ storedValue: 'throw', hasHeadings: true });
            const htmlEl = ctx._htmlEl;

            const CASE = 'init-ls-throws';
            const INIT = 'throw';

            // On storage failure the code defaults to off (saved=null → active=false).
            assert.ok(!htmlEl.classList.contains('indent-hierarchy-on'),
                `[${CASE}] init=${INIT}: root class absent when localStorage throws`);
            assert.ok(!ctx._mainEl.dataset.indentActive,
                `[${CASE}] init=${INIT}: indentActive absent when localStorage throws`);
            assert.equal(wrapperCount(ctx._mainEl), 0,
                `[${CASE}] init=${INIT}: wrappers=0 when localStorage throws`);
        }, 'init must not throw when localStorage access raises an exception');
    });

    test('8a. No-heading document initializes without throwing', (t) => {
        // planOutlineSections returns [] for no-heading pages; the IIFE must
        // not crash and the button must be bound.
        const CASE = 'no-headings-init';
        const INIT = 'off';

        assert.doesNotThrow(() => {
            makeContext({ storedValue: 'off', hasHeadings: false, hasButton: true });
        }, `[${CASE}] init=${INIT}: IIFE must not throw on no-heading document`);
    });

    test('8b. No-heading document: toggle ON creates no wrappers', (t) => {
        const ctx    = makeContext({ storedValue: 'off', hasHeadings: false, hasButton: true });
        const mainEl = ctx._mainEl;
        const htmlEl = ctx._htmlEl;

        const CASE = 'no-headings-toggle';
        const INIT = 'off';

        // Clicking ON on a no-heading page must not throw and must not create wrappers.
        assert.doesNotThrow(() => clickToggle(ctx),
            `[${CASE}] init=${INIT}: clicking toggle on no-heading page must not throw`);

        assert.equal(wrapperCount(mainEl), 0,
            `[${CASE}] init=${INIT}: wrappers=0 after toggle ON on no-heading page`);

        // The class IS toggled (state machine still advances even without headings).
        assert.ok(htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] init=${INIT}: root class set on no-heading page after toggle ON`);
    });

    test('8c. No-heading document: toggle OFF after ON does not throw', (t) => {
        const ctx  = makeContext({ storedValue: 'off', hasHeadings: false, hasButton: true });

        const CASE = 'no-headings-off';
        const INIT = 'off';

        assert.doesNotThrow(() => {
            clickToggle(ctx);  // → ON
            clickToggle(ctx);  // → OFF
        }, `[${CASE}] init=${INIT}: ON->OFF cycle on no-heading page must not throw`);

        assert.equal(wrapperCount(ctx._mainEl), 0,
            `[${CASE}] init=${INIT}: wrappers still 0 after OFF on no-heading page`);
    });

});

// ---------------------------------------------------------------------------
// Fixture helpers — outline algorithm tests (bd-1zl.7.1)
// ---------------------------------------------------------------------------

/**
 * Build a fresh vm context with an explicit list of child node definitions
 * instead of the fixed h1/h2/h3 structure used by makeContext.
 *
 * @param {Array<{tag: string, id?: string}>} childDefs
 *   Elements to append (in order) as direct children of main.content.
 * @param {object} [opts]
 * @param {string|null} [opts.storedValue='off']
 *   Initial mdmd-indent-hierarchy value (null means key absent).
 */
function makeFixtureContext(childDefs, { storedValue = 'off' } = {}) {
    const doc    = new FakeDocument();
    const htmlEl = doc.documentElement;

    const mainEl = doc.createElement('main');
    mainEl.className = 'content';
    mainEl.classList.add('content');
    htmlEl.appendChild(mainEl);

    for (const def of childDefs) {
        const el = doc.createElement(def.tag);
        if (def.id) el.id = def.id;
        mainEl.appendChild(el);
    }

    const btn = doc.createElement('button');
    btn.id = 'indent-toggle';
    htmlEl.appendChild(btn);

    const init = (storedValue !== null)
        ? { 'mdmd-indent-hierarchy': storedValue }
        : {};
    const ls = new FakeLocalStorage(init);

    // Always report reduced-motion so applyIndentOff unwraps synchronously.
    const matchMedia = () => ({ matches: true });

    const ctx = vm.createContext({
        window       : { mdmd: {} },
        document     : doc,
        localStorage : ls,
        setTimeout   : fn => fn(),
        clearTimeout : () => {},
        setInterval  : () => 0,
        clearInterval: () => {},
    });
    ctx.window.matchMedia = matchMedia;

    vm.runInContext(INDENT_SRC, ctx);

    ctx._mainEl = mainEl;
    ctx._htmlEl = htmlEl;
    ctx._btn    = btn;
    return ctx;
}

/**
 * Find the first descendant of root whose id equals the given value, or null.
 */
function findById(root, id) {
    return root._allDescendants().find(e => e.id === id) ?? null;
}

/**
 * Return the 'data-depth' attribute string of the nearest ancestor
 * data-indent-generated wrapper containing el, or null when el lives at the
 * top level of mainEl (not inside any wrapper).
 */
function wrappingDepth(el, mainEl) {
    let cur = el._parent;
    while (cur && cur !== mainEl) {
        if (cur.getAttribute('data-indent-generated') === '1') {
            return cur.getAttribute('data-depth');
        }
        cur = cur._parent;
    }
    return null;
}

// ---------------------------------------------------------------------------
// Tests — fixture-based outline algorithm (bd-1zl.7.1)
// ---------------------------------------------------------------------------
describe('indent-hierarchy — outline algorithm fixtures', () => {

    test('F1. Single H1 + paragraph → one depth-1 wrapper', () => {
        const ctx    = makeFixtureContext([
            { tag: 'h1', id: 'a' },
            { tag: 'p' },
        ]);
        const mainEl = ctx._mainEl;
        const CASE   = 'F1 single-h1';

        clickToggle(ctx);  // OFF → ON

        assert.equal(wrapperCount(mainEl), 1,
            `[${CASE}] expected exactly 1 wrapper (got ${wrapperCount(mainEl)})`);

        const h1 = findById(mainEl, 'a');
        assert.ok(h1, `[${CASE}] h1#a must be present in the DOM after ON`);
        assert.equal(wrappingDepth(h1, mainEl), '1',
            `[${CASE}] h1#a must live inside a depth-1 wrapper`);

        // The generated wrapper must be a direct child of main.
        const wrapper = mainEl.querySelector('[data-indent-generated="1"]');
        assert.ok(wrapper, `[${CASE}] generated wrapper must be findable`);
        assert.equal(wrapper._parent, mainEl,
            `[${CASE}] wrapper must be a direct child of main.content`);
        assert.equal(wrapper.getAttribute('data-depth'), '1',
            `[${CASE}] wrapper data-depth must be "1"`);
    });

    test('F2. Mixed levels H1/H2/H3 → nested wrappers at depths 1/2/3', () => {
        const ctx    = makeFixtureContext([
            { tag: 'h1', id: 'a' },
            { tag: 'h2', id: 'b' },
            { tag: 'h3', id: 'c' },
        ]);
        const mainEl = ctx._mainEl;
        const CASE   = 'F2 mixed-levels';

        clickToggle(ctx);  // OFF → ON

        assert.equal(wrapperCount(mainEl), 3,
            `[${CASE}] expected 3 wrappers — one per heading (got ${wrapperCount(mainEl)})`);

        const h1 = findById(mainEl, 'a');
        const h2 = findById(mainEl, 'b');
        const h3 = findById(mainEl, 'c');

        assert.equal(wrappingDepth(h1, mainEl), '1',
            `[${CASE}] h1#a must be in a depth-1 wrapper`);
        assert.equal(wrappingDepth(h2, mainEl), '2',
            `[${CASE}] h2#b must be in a depth-2 wrapper`);
        assert.equal(wrappingDepth(h3, mainEl), '3',
            `[${CASE}] h3#c must be in a depth-3 wrapper`);

        // Structural nesting: depth-2 wrapper must be inside depth-1 wrapper.
        const d2Wrapper = h2._parent;
        assert.equal(d2Wrapper.getAttribute('data-depth'), '2',
            `[${CASE}] h2's immediate parent must be the depth-2 wrapper`);
        assert.equal(d2Wrapper._parent.getAttribute('data-depth'), '1',
            `[${CASE}] depth-2 wrapper must be nested inside the depth-1 wrapper`);

        // depth-1 wrapper must be a direct child of main.
        const d1Wrapper = d2Wrapper._parent;
        assert.equal(d1Wrapper._parent, mainEl,
            `[${CASE}] depth-1 wrapper must be a direct child of main.content`);
    });

    test('F3. Skipped level H1→H3 → H3 nested at depth 2 (no phantom H2)', () => {
        const ctx    = makeFixtureContext([
            { tag: 'h1', id: 'a' },
            { tag: 'h3', id: 'c' },
        ]);
        const mainEl = ctx._mainEl;
        const CASE   = 'F3 skipped-level';

        clickToggle(ctx);

        assert.equal(wrapperCount(mainEl), 2,
            `[${CASE}] expected 2 wrappers for H1+H3 (got ${wrapperCount(mainEl)})`);

        const h3 = findById(mainEl, 'c');
        assert.equal(wrappingDepth(h3, mainEl), '2',
            `[${CASE}] h3#c must be at depth 2 despite the skipped H2 ` +
            `(actual: ${wrappingDepth(h3, mainEl)})`);

        // The depth-2 wrapper must be nested inside the depth-1 wrapper.
        const d2Wrapper = h3._parent;
        assert.equal(d2Wrapper.getAttribute('data-depth'), '2',
            `[${CASE}] h3's parent must have data-depth="2"`);
        assert.equal(d2Wrapper._parent.getAttribute('data-depth'), '1',
            `[${CASE}] depth-2 wrapper must be inside the depth-1 wrapper`);
    });

    test('F4. Pre-heading content → stays unwrapped as direct child of main', () => {
        const ctx    = makeFixtureContext([
            { tag: 'p',  id: 'pre'  },
            { tag: 'h1', id: 'a'    },
            { tag: 'p',  id: 'post' },
        ]);
        const mainEl = ctx._mainEl;
        const CASE   = 'F4 pre-heading-content';

        clickToggle(ctx);

        assert.equal(wrapperCount(mainEl), 1,
            `[${CASE}] expected 1 wrapper (h1 section only, got ${wrapperCount(mainEl)})`);

        // p#pre must remain a direct child of main (depth-0 content).
        const pre = findById(mainEl, 'pre');
        assert.ok(pre, `[${CASE}] p#pre must exist in the DOM`);
        assert.equal(pre._parent, mainEl,
            `[${CASE}] p#pre must be a direct child of main.content (not wrapped)`);

        // p#post must be inside the generated wrapper.
        const post = findById(mainEl, 'post');
        assert.ok(post, `[${CASE}] p#post must exist in the DOM`);
        assert.notEqual(post._parent, mainEl,
            `[${CASE}] p#post must NOT be a direct child of main (must be inside wrapper)`);
        assert.equal(post._parent.getAttribute('data-indent-generated'), '1',
            `[${CASE}] p#post's parent must be a generated indent-section wrapper`);
    });

    test('F5. No headings → transform is a no-op; zero wrappers created', () => {
        const ctx    = makeFixtureContext([
            { tag: 'p' },
            { tag: 'p' },
        ]);
        const mainEl = ctx._mainEl;
        const htmlEl = ctx._htmlEl;
        const CASE   = 'F5 no-headings';

        assert.doesNotThrow(() => clickToggle(ctx),
            `[${CASE}] toggle ON on a no-heading page must not throw`);

        assert.equal(wrapperCount(mainEl), 0,
            `[${CASE}] zero wrappers must be created when there are no headings`);

        // The root class IS still applied — the state machine advances normally.
        assert.ok(htmlEl.classList.contains('indent-hierarchy-on'),
            `[${CASE}] root class 'indent-hierarchy-on' must be set even with no headings`);
    });

    test('F6. Same-level H2 sequence → sibling sections, all at depth 1', () => {
        const ctx    = makeFixtureContext([
            { tag: 'h2', id: 'a'  },
            { tag: 'p',  id: 'p1' },
            { tag: 'h2', id: 'b'  },
            { tag: 'p',  id: 'p2' },
            { tag: 'h2', id: 'c'  },
            { tag: 'p',  id: 'p3' },
        ]);
        const mainEl = ctx._mainEl;
        const CASE   = 'F6 same-level-siblings';

        clickToggle(ctx);

        assert.equal(wrapperCount(mainEl), 3,
            `[${CASE}] expected 3 sibling wrappers for 3 H2 elements (got ${wrapperCount(mainEl)})`);

        // All wrappers must be depth-1 direct children of main — no nesting.
        const wrappers = mainEl.querySelectorAll('[data-indent-generated="1"]');
        for (const w of wrappers) {
            assert.equal(w.getAttribute('data-depth'), '1',
                `[${CASE}] every wrapper must have data-depth="1" for same-level H2s`);
            assert.equal(w._parent, mainEl,
                `[${CASE}] every wrapper must be a direct child of main.content`);
        }

        // Each heading must land in its own depth-1 wrapper.
        for (const id of ['a', 'b', 'c']) {
            const el = findById(mainEl, id);
            assert.equal(wrappingDepth(el, mainEl), '1',
                `[${CASE}] h2#${id} must be inside a depth-1 wrapper`);
        }

        // No accidental nesting: all three wrappers must be distinct elements.
        assert.equal(new Set(wrappers).size, 3,
            `[${CASE}] the 3 wrappers must be distinct elements (no duplication)`);
    });

});
