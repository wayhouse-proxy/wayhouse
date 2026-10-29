import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { ApiError, clearNodeTags, setNodeTags } from "../../api";
import type { FleetNode } from "../../lib/fleet/nodes";
import { useCan } from "../../lib/session";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { useToast } from "../ui/Toast";

const KEY = /^[a-z0-9._-]{1,64}$/;
const VALUE = /^[A-Za-z0-9._:/@+-]{1,64}$/;
const MAX_TAGS = 16;

/**
 * A node's tags. What the node reports can only be filtered on; the operator's part is editable and
 * overrides a reported tag of the same key. Each change sends the whole operator set (docs/06).
 */
export function NodeTags({
  instance,
  node,
  unavailable,
  onTag,
}: {
  instance: string;
  node?: FleetNode;
  /** The controller did not answer the list, so the operator's part is not known. */
  unavailable?: boolean;
  onTag: (tag: string) => void;
}) {
  const canOperate = useCan("operator");
  const toast = useToast();
  const qc = useQueryClient();
  const [key, setKey] = useState("");
  const [value, setValue] = useState("");
  const [problem, setProblem] = useState("");

  const mine = node?.operator_tags ?? {};
  const reported = Object.entries(node?.tags ?? {}).filter(([k]) => !(k in mine));
  // The controller addresses a node by one URL segment of at most 64 characters.
  const untaggable = instance.includes("/") || instance.length > 64;
  const off = !canOperate || unavailable || untaggable || !node;

  const save = useMutation({
    mutationFn: (tags: Record<string, string>) => (Object.keys(tags).length ? setNodeTags(instance, tags) : clearNodeTags(instance)),
    onSuccess: () => {
      setKey("");
      setValue("");
      return qc.invalidateQueries({ queryKey: ["fleet", "nodes"] });
    },
    onError: (e) => toast(`Tags not saved: ${e instanceof ApiError ? e.message : e}`, "down"),
  });

  function add() {
    if (!KEY.test(key)) return setProblem("A key uses lowercase letters, digits and . _ - (at most 64).");
    if (!VALUE.test(value)) return setProblem("A value uses letters, digits and . _ : / @ + - (1 to 64).");
    if (!(key in mine) && Object.keys(mine).length >= MAX_TAGS) return setProblem(`At most ${MAX_TAGS} tags.`);
    setProblem("");
    save.mutate({ ...mine, [key]: value });
  }
  function remove(k: string) {
    const { [k]: _gone, ...rest } = mine;
    save.mutate(rest);
  }

  const chip = "rounded-sm border border-line px-1.5 py-0.5 font-mono text-xs";
  return (
    <section aria-label="Tags" className="flex flex-col gap-2">
      <h3 className="font-semibold">Tags</h3>
      {unavailable && <p className="text-ink-muted">Operator tags are unavailable (the controller did not answer), so only the tags the node reports are shown and editing is off.</p>}
      {untaggable && <p className="text-ink-muted">A node id with a slash or over 64 characters cannot be tagged.</p>}
      <ul className="flex flex-wrap gap-1">
        {reported.map(([k, v]) => (
          <li key={k}>
            <button type="button" title="Filter by this tag (reported by the node)" onClick={() => onTag(`${k}=${v}`)} className={`${chip} hover:bg-surface-sunken`}>
              {k}={v}
            </button>
          </li>
        ))}
        {Object.entries(mine).map(([k, v]) => (
          <li key={k} className="flex">
            <button type="button" title="Filter by this tag (set by an operator)" onClick={() => onTag(`${k}=${v}`)} className={`${chip} rounded-r-none bg-surface-sunken hover:bg-surface-raised`}>
              {k}={v}
            </button>
            <button type="button" aria-label={`Remove tag ${k}`} disabled={off || save.isPending} onClick={() => remove(k)} className={`${chip} rounded-l-none border-l-0 disabled:opacity-50`}>
              ×
            </button>
          </li>
        ))}
        {reported.length === 0 && Object.keys(mine).length === 0 && <li className="text-ink-muted">None. A node reports its own with <code className="font-mono">--tag key=value</code>.</li>}
      </ul>
      <div className="flex gap-1">
        <Input mono aria-label="Tag key" placeholder="key" value={key} disabled={off} onChange={(e) => setKey(e.target.value)} />
        <Input mono aria-label="Tag value" placeholder="value" value={value} disabled={off} onChange={(e) => setValue(e.target.value)} />
        <Button disabled={off || save.isPending || !key || !value} onClick={add}>
          Add tag
        </Button>
      </div>
      {problem && <p role="alert" className="text-bad">{problem}</p>}
    </section>
  );
}
