const EDITABLE_TAG_NAMES = new Set(['INPUT', 'TEXTAREA', 'SELECT']);

interface ElementLike {
  readonly tagName?: string;
  readonly isContentEditable?: boolean;
  getAttribute?(name: string): string | null;
  closest?(selector: string): Element | null;
}

export function isEditableElement(target: EventTarget | null): boolean {
  if (target === null || typeof target !== 'object') {
    return false;
  }

  const el = target as ElementLike;
  if (typeof el.tagName === 'string' && EDITABLE_TAG_NAMES.has(el.tagName.toUpperCase())) {
    return true;
  }

  if (el.isContentEditable === true) {
    return true;
  }

  if (typeof el.getAttribute === 'function') {
    const contentEditableAttr = el.getAttribute('contenteditable');
    if (
      contentEditableAttr !== null &&
      (contentEditableAttr === '' ||
        contentEditableAttr.toLowerCase() === 'true' ||
        contentEditableAttr.toLowerCase() === 'plaintext-only')
    ) {
      return true;
    }
  }

  if (typeof el.closest === 'function') {
    try {
      const editableAncestor = el.closest(
        '[contenteditable="true"], [contenteditable=""], [contenteditable="plaintext-only"]',
      );
      if (editableAncestor !== null) {
        return true;
      }
    } catch {
      // Ignore selector errors on non-standard nodes.
    }
  }

  return false;
}

export interface ShortcutKeyEventLike {
  readonly key: string;
  readonly ctrlKey: boolean;
  readonly metaKey: boolean;
  readonly altKey: boolean;
  readonly shiftKey: boolean;
  readonly target: EventTarget | null;
}

/**
 * Returns true when the user invokes Ctrl+V (or Cmd+V on macOS) outside of any editable field
 * so the DeskApp can open and focus the explicit Draft Drawer textarea without hijacking normal
 * paste inside inputs, textareas, selects, or contentEditable regions.
 */
export function isDraftDrawerPasteShortcut(
  event: ShortcutKeyEventLike,
  activeElement: EventTarget | null = typeof document !== 'undefined'
    ? document.activeElement
    : null,
): boolean {
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) {
    return false;
  }
  if (event.key.toLowerCase() !== 'v') {
    return false;
  }
  if (isEditableElement(event.target) || isEditableElement(activeElement)) {
    return false;
  }
  return true;
}
