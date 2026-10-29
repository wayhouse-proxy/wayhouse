import { useCallback, useEffect, useMemo, useState } from "react";
import { ApiError, getConfigSchema, getCurrentConfig, submitConfig, validateConfig } from "../../api";
import { diff, parseText, patchText } from "./patch";

export interface ConfigDoc {
  /** The YAML as it stands in the editor. */
  text: string;
  /** The YAML as last saved on the controller. */
  savedText: string;
  /** The saved revision this edit is based on; null when none was submitted yet. */
  revision: number | null;
  /** The document as plain values (null while the text does not parse). */
  value: unknown;
  savedValue: unknown;
  parseError: string | null;
  schema: Record<string, any> | null;
  loading: boolean;
  error: string | null;
  /** The number of changed values, for the save bar. */
  changes: number;
  /** Replace the whole document value (what SchemaForm gives back); comments and unknown keys stay. */
  setValue: (next: unknown) => void;
  /** Replace the YAML text (the Source page). */
  setText: (text: string) => void;
  discard: () => void;
  reload: () => Promise<void>;
  validate: () => Promise<{ ok: boolean; message?: string; schemaVersion?: number }>;
  /** Submits the edit based on the loaded revision; a 409 means someone else changed it (`stale`). */
  apply: () => Promise<{ ok: true; revision: number } | { ok: false; stale: boolean; message: string }>;
}

/**
 * The configuration as the editor works on it: the YAML text is the truth and form edits are patches to it,
 * so comments and keys the forms do not know survive an edit. Every form page, the Source page and the
 * domain editors share this.
 */
export function useConfigDoc(): ConfigDoc {
  const [savedText, setSavedText] = useState("");
  const [text, setText] = useState("");
  const [revision, setRevision] = useState<number | null>(null);
  const [schema, setSchema] = useState<Record<string, any> | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      const [current, sch] = await Promise.all([
        getCurrentConfig().catch((e) => {
          if (e instanceof ApiError && e.status === 404) return { text: "", revision: null };
          throw e;
        }),
        getConfigSchema(),
      ]);
      setSavedText(current.text);
      setText(current.text);
      setRevision(current.revision === null ? null : Number(current.revision));
      setSchema(sch);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);

  const parsed = useMemo(() => parseText(text), [text]);
  const saved = useMemo(() => parseText(savedText), [savedText]);
  const changes = useMemo(() => (parsed.error ? 0 : diff(saved.value, parsed.value).length), [parsed, saved]);

  const setValue = useCallback(
    (next: unknown) =>
      setText((t) => {
        const { value, error } = parseText(t);
        return error ? t : patchText(t, value, next); // the YAML is broken: leave it to the Source page
      }),
    [],
  );

  const validate = useCallback(async () => {
    try {
      const r = await validateConfig(text);
      return { ok: true, schemaVersion: r.schema_version };
    } catch (e) {
      return { ok: false, message: e instanceof Error ? e.message : String(e) };
    }
  }, [text]);

  const apply = useCallback(async () => {
    try {
      const r = await submitConfig(text, revision ?? 0);
      setSavedText(text);
      setRevision(r.revision);
      return { ok: true as const, revision: r.revision };
    } catch (e) {
      return { ok: false as const, stale: e instanceof ApiError && e.status === 409, message: e instanceof Error ? e.message : String(e) };
    }
  }, [text, revision]);

  return {
    text, savedText, revision, value: parsed.value, savedValue: saved.value, parseError: parsed.error, schema,
    loading, error, changes, setValue, setText, discard: () => setText(savedText), reload, validate, apply,
  };
}
