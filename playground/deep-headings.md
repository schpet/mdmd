# Deep Headings

← [Back to index](README.md)

A stress-test for indentation hierarchy. H1 and H2 are flush; H3 and deeper receive progressive indentation. Toggle the indent button (top-right) to compare modes.

## Chapter One

Content before any sub-section.

### Section 1.1

Some text in 1.1.

#### Subsection 1.1.1

Text in 1.1.1.

##### Detail 1.1.1.1

Text at depth 5.

###### Note 1.1.1.1.a

Text at depth 6 — the deepest standard heading.

##### Detail 1.1.1.2

Sibling of 1.1.1.1.

#### Subsection 1.1.2

Sibling of 1.1.1.

### Section 1.2

Sibling of 1.1.

#### Subsection 1.2.1

Text in 1.2.1.

##### Detail 1.2.1.1

Deep again.

###### Note 1.2.1.1.a

Six levels in.

## Chapter Two

A fresh top-level section to confirm H2 resets to flush.

### Section 2.1

#### Subsection 2.1.1

##### Detail 2.1.1.1

###### Note 2.1.1.1.a

Deepest in chapter two.

### Section 2.2

Sibling section; confirms H3 indent is consistent across chapters.

#### Subsection 2.2.1

#### Subsection 2.2.2

Two subsections side by side to show same-depth alignment.

## Chapter Three — Skipped Levels

This chapter jumps from H2 directly to H4 and H6 to verify the outline algorithm handles non-contiguous levels gracefully.

#### Jump to H4

No H3 above this.

###### Jump to H6

No H5 above this.

### Now H3

Heading level steps back up; should still indent relative to H2.

## Chapter Four — Alternating Depths

### H3

#### H4

### H3 again

#### H4 again

##### H5

### H3 one more time

Short prose paragraph to give each section a bit of breathing room and make the indent cascade easier to read visually.
