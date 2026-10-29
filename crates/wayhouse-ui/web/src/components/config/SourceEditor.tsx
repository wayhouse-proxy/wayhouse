import { useEffect, useRef } from "react";
import { basicSetup, EditorView } from "codemirror";
import { yaml } from "@codemirror/lang-yaml";
import { EditorState } from "@codemirror/state";

/**
 * The YAML as text (CodeMirror 6). Controlled loosely: the editor owns typing, and `value` replaces its
 * content only when it differs from what the editor already holds (a Discard or a reload).
 */
export default function SourceEditor({ value, onChange, readOnly, label }: { value: string; onChange: (text: string) => void; readOnly?: boolean; label: string }) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const change = useRef(onChange);
  change.current = onChange;

  useEffect(() => {
    const v = new EditorView({
      parent: host.current!,
      state: EditorState.create({
        doc: value,
        extensions: [
          basicSetup,
          yaml(),
          EditorState.readOnly.of(!!readOnly),
          EditorView.contentAttributes.of({ "aria-label": label }),
          EditorView.updateListener.of((u) => u.docChanged && change.current(u.state.doc.toString())),
        ],
      }),
    });
    view.current = v;
    return () => v.destroy();
  }, [readOnly]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const v = view.current;
    if (v && v.state.doc.toString() !== value) v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: value } });
  }, [value]);

  return <div ref={host} className="min-h-80 border border-line text-sm [&_.cm-editor]:min-h-80 [&_.cm-scroller]:font-mono" />;
}
