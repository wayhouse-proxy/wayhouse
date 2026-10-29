import { useEffect } from "react";
import type { StoredTheme } from "../../api";
import { BUILTIN, effective } from "../../lib/theme/builtin";
import { useTheme } from "../../lib/theme/provider";
import { nameProblem, TOKEN_GROUPS, withToken } from "../../lib/theme/store";
import { validateTheme } from "../../lib/theme/validate";
import { Button } from "../ui/Button";
import { FieldRow } from "../ui/FieldRow";
import { Input, Select } from "../ui/Input";
import { Section } from "../ui/Section";
import { StatusMark } from "../ui/StatusMark";

const HEX = /^#[0-9a-f]{6}$/i;

/**
 * Edits one custom theme: each token with its value and the checks it takes part in (themes.md, Validation).
 * The draft is previewed on the whole UI while the editor is open; Save needs a name and every check passing.
 */
export function ThemeEditor({
  draft,
  onChange,
  onSave,
  onCancel,
  taken,
  canWrite,
  saving,
}: {
  draft: StoredTheme;
  onChange: (t: StoredTheme) => void;
  onSave: () => void;
  onCancel: () => void;
  /** Names that exist already, other than this theme's own. */
  taken: string[];
  canWrite: boolean;
  saving?: boolean;
}) {
  const { preview } = useTheme();
  useEffect(() => {
    preview(draft);
    return () => preview(null);
  }, [draft, preview]);

  const failures = validateTheme(draft);
  const nameError = nameProblem(draft.name, taken);
  const eff = effective(draft);
  const base = BUILTIN[draft.base];
  const blocked = !!nameError || failures.length > 0 || !canWrite;

  return (
    <section aria-label="Theme editor" className="mt-6">
      <h2 className="mb-2 text-lg font-semibold">{draft.name || "New theme"}</h2>
      <FieldRow label="Name" required error={nameError ?? undefined} render={(a) => <Input {...a} value={draft.name} onChange={(e) => onChange({ ...draft, name: e.target.value })} />} />
      <FieldRow
        label="Starts from"
        help="Tokens you leave alone follow this built-in theme."
        render={(a) => (
          <Select {...a} value={draft.base} onChange={(e) => onChange({ ...draft, base: e.target.value as "light" | "dark" })}>
            <option value="light">Light</option>
            <option value="dark">Dark</option>
          </Select>
        )}
      />
      {TOKEN_GROUPS.map((g) => (
        <Section key={g.label} title={g.label} changed={g.tokens.filter((t) => t in draft.tokens).length}>
          {g.tokens.map((token) => {
            const mine = failures.filter((f) => f.pair.includes(token));
            const value = eff[token];
            return (
              <FieldRow
                key={token}
                label={token}
                changed={token in draft.tokens}
                error={mine.length ? mine.map((f) => f.message).join(" ") : undefined}
                render={(a) => (
                  <div className="flex flex-wrap items-center gap-2">
                    <input
                      type="color"
                      aria-label={`${token} picker`}
                      value={HEX.test(value) ? value : "#000000"}
                      onChange={(e) => onChange(withToken(draft, token, e.target.value, base[token]))}
                      className="h-8 w-10 rounded-sm border border-control bg-transparent p-0.5"
                    />
                    <Input {...a} mono value={value} onChange={(e) => onChange(withToken(draft, token, e.target.value, base[token]))} className="w-28" />
                    {!mine.length && <StatusMark state="up" word="Passes" />}
                    {token in draft.tokens && (
                      <Button size="sm" variant="ghost" aria-label={`Reset ${token} to ${draft.base}`} onClick={() => onChange(withToken(draft, token, base[token], base[token]))}>
                        Reset to base
                      </Button>
                    )}
                  </div>
                )}
              />
            );
          })}
        </Section>
      ))}
      <div className="sticky bottom-0 mt-4 flex items-center justify-end gap-2 border-t border-line bg-bg py-2">
        {failures.length > 0 && <span role="status" className="mr-auto text-sm text-bad">{failures.length} check{failures.length === 1 ? "" : "s"} failing: the theme cannot be saved yet.</span>}
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="primary" onClick={onSave} disabled={blocked} loading={saving} title={canWrite ? undefined : "Saving a theme needs the admin role"}>
          Save theme
        </Button>
      </div>
    </section>
  );
}
