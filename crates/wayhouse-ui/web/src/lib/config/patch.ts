import { isMap, isScalar, isSeq, parseDocument, type Document } from "yaml";

export type Path = (string | number)[];
type Op = { kind: "set"; path: Path; value: unknown } | { kind: "delete"; path: Path };

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);

/**
 * The minimal set and delete operations that turn `before` into `after`. Unchanged values
 * produce nothing, which is what keeps their comments, quoting and key order in the file.
 * An `undefined` value (a cleared field) deletes the key.
 */
export function diff(before: unknown, after: unknown, path: Path = [], ops: Op[] = []): Op[] {
  if (isObject(before) && isObject(after)) {
    for (const [k, v] of Object.entries(after)) {
      if (v === undefined) {
        if (k in before) ops.push({ kind: "delete", path: [...path, k] });
      } else if (!(k in before)) ops.push({ kind: "set", path: [...path, k], value: v });
      else diff(before[k], v, [...path, k], ops);
    }
    for (const k of Object.keys(before)) if (!(k in after)) ops.push({ kind: "delete", path: [...path, k] });
  } else if (Array.isArray(before) && Array.isArray(after)) {
    const common = Math.min(before.length, after.length);
    for (let i = 0; i < common; i++) diff(before[i], after[i], [...path, i], ops);
    for (let i = common; i < after.length; i++) ops.push({ kind: "set", path: [...path, i], value: after[i] });
    for (let i = before.length - 1; i >= common; i--) ops.push({ kind: "delete", path: [...path, i] });
  } else if (JSON.stringify(before) !== JSON.stringify(after)) {
    ops.push({ kind: "set", path, value: after });
  }
  return ops;
}

function apply(doc: Document, ops: Op[]) {
  for (const op of ops) {
    if (op.kind === "delete") {
      doc.deleteIn(op.path);
      continue;
    }
    if (op.path.length === 0) {
      doc.contents = doc.createNode(op.value);
      continue;
    }
    // A missing parent (a new `settings.admin`) is created by setIn; a null one (a key with no value) is replaced.
    const parent = doc.getIn(op.path.slice(0, -1), true);
    if (parent !== undefined && !isMap(parent) && !isSeq(parent) && op.path.length > 1)
      doc.setIn(op.path.slice(0, -1), doc.createNode({}));
    // A scalar is updated in place so its trailing comment stays with it.
    const node = doc.getIn(op.path, true);
    if (isScalar(node) && (op.value === null || typeof op.value !== "object")) node.value = op.value;
    else doc.setIn(op.path, doc.createNode(op.value));
  }
}

/**
 * Applies the form's edit (`before` to `after`, both plain values) to the YAML text and returns the new
 * text. Comments, key order and keys the form does not know are kept; the writer normalises spacing
 * inside a line (one space before a trailing comment).
 * shortcut: a sequence item inserted in the middle shifts the items after it, so their comments move with the index, not the item; fine for the forms, revisit if a list gets per-item comments in practice.
 */
export function patchText(text: string, before: unknown, after: unknown): string {
  const ops = diff(before, after);
  if (ops.length === 0) return text;
  const doc: Document = parseDocument(text);
  if (doc.errors.length) throw new Error(doc.errors[0].message);
  if (doc.contents === null) doc.contents = doc.createNode({});
  apply(doc, ops);
  return doc.toString({ flowCollectionPadding: false });
}

export function parseText(text: string): { value: unknown; error: string | null } {
  const doc = parseDocument(text);
  if (doc.errors.length) return { value: null, error: doc.errors[0].message };
  return { value: doc.toJS() ?? {}, error: null };
}
