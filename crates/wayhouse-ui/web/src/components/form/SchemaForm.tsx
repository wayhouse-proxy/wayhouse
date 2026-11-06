import { useEffect, useMemo, useState } from "react";
import Form from "@rjsf/core";
import { customizeValidator } from "@rjsf/validator-ajv8";
import Ajv2020 from "ajv/dist/2020";
import { ADDITIONAL_PROPERTY_FLAG } from "@rjsf/utils";
import type {
  ArrayFieldItemTemplateProps,
  ArrayFieldTemplateProps,
  FieldTemplateProps,
  ObjectFieldTemplateProps,
  RegistryWidgetsType,
  RJSFSchema,
  UiSchema,
  WidgetProps,
} from "@rjsf/utils";
import { Plus } from "lucide-react";
import { putClusterSecret } from "../../api";
import { suggestSecretName, validSecretName } from "../../lib/secretName";
import { diff } from "../../lib/config/patch";
import { get, set, type Path } from "../../lib/form/path";
import { DEFAULT_LAYOUT, patternOf, type FieldLayout, type Layout } from "../../lib/form/layout";
import { coerce, resolve, simplify, sinceOf, subschema, unitFor } from "../../lib/form/schema";
import { Button } from "../ui/Button";
import { FieldRow } from "../ui/FieldRow";
import { Input, NumberInput, Select, Textarea } from "../ui/Input";
import { Section } from "../ui/Section";
import { Switch } from "../ui/Switch";

const validator = customizeValidator({ AjvClass: Ajv2020 });

interface Ctx {
  layout: Layout;
  saved: unknown;
  current: unknown;
  root: Path;
  showAdvanced: boolean;
  page?: string;
  only?: string[];
}

const lay = (ctx: Ctx, path: Path): FieldLayout | undefined => ctx.layout.fields[patternOf([...ctx.root, ...path])];
const absolute = (ctx: Ctx, path: Path) => [...ctx.root, ...path];
const changedCount = (ctx: Ctx, path: Path) => diff(get(ctx.saved, absolute(ctx, path)), get(ctx.current, absolute(ctx, path))).length;
const helpOf = (text?: string) => text?.split("\n\n")[0].replace(/\s+/g, " ").trim();

/** Whether a field, or anything under it, is on `page` in the layout. */
function onPage(ctx: Ctx, path: Path): boolean {
  if (ctx.only && path.length > 0 && !ctx.only.includes(String(path[0]))) return false;
  if (!ctx.page) return true;
  const prefix = patternOf(absolute(ctx, path));
  return Object.entries(ctx.layout.fields).some(([k, f]) => f.page === ctx.page && (k === prefix || k.startsWith(`${prefix}.`)));
}

/** Machine data (addresses, paths, keys) is set in mono; ordinary text, names and groups are not. Whole words of the field name only. */
const MONO_WORDS = new Set(["listen", "bind", "addr", "address", "url", "dir", "path", "db", "cert", "key", "target", "seed", "host", "cidr", "pubkey"]);
const isMono = (name: string) => name.split(/[^a-z0-9]+/i).some((w) => MONO_WORDS.has(w.toLowerCase()));

/**
 * Widgets point at their help text, but only when there is some: `uiFor` records that in the `help` option.
 * (The server answers a bad document with one message, shown by the save bar, so there are no per-field errors.)
 */
function describe(p: WidgetProps) {
  return { "aria-describedby": p.options.help ? `${p.id}-help` : undefined };
}

/** `{ secret: name }` as written in the YAML; the form shows it but does not edit it until the secret picker exists. */
const secretRefName = (v: unknown): string | undefined =>
  typeof v === "object" && v !== null && typeof (v as { secret?: unknown }).secret === "string" ? (v as { secret: string }).secret : undefined;

/** A value that is not text (today only a `{ secret }` reference) is shown, not edited: typing would overwrite it. */
const notText = (v: unknown) => v !== undefined && v !== null && typeof v !== "string";
const RefShown = (p: WidgetProps) => (
  <Input id={p.id} readOnly mono value={secretRefName(p.value) ? `secret: ${secretRefName(p.value)}` : "(set in the YAML)"} {...describe(p)} />
);

const Text = (p: WidgetProps) =>
  notText(p.value) ? (
    <RefShown {...p} />
  ) : (
    <Input
      id={p.id}
      value={p.value ?? ""}
      mono={isMono(p.name ?? p.label)}
      placeholder={p.schema["x-wayhouse-default"] !== undefined ? String(p.schema["x-wayhouse-default"]) : undefined}
      disabled={p.disabled || p.readonly}
      onChange={(e) => p.onChange(e.target.value === "" ? undefined : e.target.value)}
      onBlur={() => p.onBlur(p.id, p.value)}
      {...describe(p)}
    />
  );

function Secret(p: WidgetProps) {
  if (notText(p.value)) return <RefShown {...p} />;
  return (
    <div className="flex flex-col gap-1.5">
      <Input
        id={p.id}
        type="password"
        autoComplete="off"
        value={p.value ?? ""}
        mono
        onChange={(e) => p.onChange(e.target.value === "" ? undefined : e.target.value)}
        {...describe(p)}
      />
      {typeof p.value === "string" && p.value !== "" && !p.disabled && !p.readonly && (
        <MoveToStore field={p.name ?? p.label} value={p.value} onMoved={(name) => p.onChange({ secret: name })} />
      )}
    </div>
  );
}

/** Stores a plaintext value in the cluster secret store and swaps the field for a reference to it. */
function MoveToStore({ field, value, onMoved }: { field: string; value: string; onMoved: (name: string) => void }) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState(suggestSecretName(field));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  if (!open)
    return (
      <div>
        <Button type="button" variant="ghost" onClick={() => setOpen(true)}>
          Move to secret store
        </Button>
      </div>
    );
  async function move() {
    setBusy(true);
    setError(null);
    try {
      await putClusterSecret(name, value);
      onMoved(name);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  }
  return (
    <div className="flex flex-col gap-1.5 rounded-sm border border-line p-2">
      <label className="flex flex-col gap-1 text-sm text-ink-muted">
        Secret name
        <Input mono value={name} onChange={(e) => setName(e.target.value)} />
      </label>
      <p className="text-sm text-ink-muted">Saves the value to the controller and puts {"{ secret: name }"} here. A reference needs schema_version 4 and replaces a stored secret of the same name.</p>
      {error && <p role="alert" className="text-sm text-bad">{error}</p>}
      <div className="flex gap-2">
        <Button type="button" variant="primary" loading={busy} disabled={!validSecretName(name)} onClick={move}>
          Store and replace
        </Button>
        <Button type="button" variant="ghost" onClick={() => setOpen(false)}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

/**
 * A number is edited as text. A draft that is empty or not yet a number (a cleared field, a lone "-") writes nothing,
 * so retyping never deletes the key and its comment; leaving the field empty is what removes the key.
 */
function NumberW(p: WidgetProps) {
  const unit = p.options.unit as string | undefined;
  const show = (v: unknown) => (v === undefined || v === null ? "" : String(v));
  const [text, setText] = useState(show(p.value));
  useEffect(() => setText((t) => (t !== "" && Number(t) === p.value ? t : t === "" && p.value === undefined ? t : show(p.value))), [p.value]);
  return (
    <NumberInput
      id={p.id}
      unit={unit}
      value={text}
      placeholder={p.schema["x-wayhouse-default"] !== undefined ? String(p.schema["x-wayhouse-default"]) : undefined}
      min={p.schema.minimum}
      step={p.schema.type === "integer" ? 1 : "any"}
      disabled={p.disabled || p.readonly}
      onChange={(e) => {
        setText(e.target.value);
        if (e.target.value !== "" && Number.isFinite(Number(e.target.value))) p.onChange(Number(e.target.value));
      }}
      onBlur={(e) => {
        // A type=number input reports "" for an unparsable draft ("-", "1e"): that is not an emptied field. Restore the last valid value instead of deleting the key.
        if (e.currentTarget.validity.badInput) setText(show(p.value));
        else if (text === "" && p.value !== undefined) p.onChange(undefined);
      }}
      {...describe(p)}
    />
  );
}

const Bool = (p: WidgetProps) => (
  <Switch id={p.id} checked={!!p.value} onCheckedChange={(v) => p.onChange(v)} disabled={p.disabled || p.readonly} {...describe(p)} />
);

const Choice = (p: WidgetProps) => (
  <Select id={p.id} value={p.value ?? ""} disabled={p.disabled || p.readonly} onChange={(e) => p.onChange(e.target.value === "" ? undefined : e.target.value)} {...describe(p)}>
    {!p.required && <option value="">{p.schema["x-wayhouse-default"] !== undefined ? `Default (${String(p.schema["x-wayhouse-default"])})` : "Not set"}</option>}
    {(p.options.enumOptions ?? []).map((o) => (
      <option key={String(o.value)} value={String(o.value)}>
        {String(o.label)}
      </option>
    ))}
  </Select>
);

const toLines = (text: string) => text.split("\n").map((l) => l.trim()).filter(Boolean);

/** A list of strings, one per line. The text is kept as typed (a new empty line must survive) and re-read only when the list changes elsewhere. */
function Lines(p: WidgetProps) {
  const list: string[] = p.value ?? [];
  const [text, setText] = useState(list.join("\n"));
  useEffect(() => {
    setText((t) => (JSON.stringify(toLines(t)) === JSON.stringify(list) ? t : list.join("\n")));
  }, [list]);
  return (
    <Textarea
      id={p.id}
      rows={Math.max(3, text.split("\n").length + 1)}
      spellCheck={false}
      disabled={p.disabled || p.readonly}
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        const lines = toLines(e.target.value);
        p.onChange(lines.length ? lines : undefined);
      }}
      className="w-full max-w-lg"
      {...describe(p)}
    />
  );
}

const WIDGETS: RegistryWidgetsType = {
  TextWidget: Text,
  NumberWidget: NumberW,
  UpDownWidget: NumberW,
  CheckboxWidget: Bool,
  SelectWidget: Choice,
  secret: Secret,
  lines: Lines,
};
const WIDGET_NAMES: Record<string, string> = { secret: "secret", lines: "lines", select: "SelectWidget", switch: "CheckboxWidget", number: "NumberWidget", text: "TextWidget" };

function FieldTemplate(p: FieldTemplateProps) {
  const ctx = p.registry.formContext as Ctx;
  const path = p.fieldPathId.path;
  const l = lay(ctx, path);
  if (p.hidden) return p.children;
  if (l?.advanced && !ctx.showAdvanced) return null;
  if ((p.schema as any)[ADDITIONAL_PROPERTY_FLAG]) return <MapEntry {...p} />;
  if (p.schema.type === "object" || (p.schema.type === "array" && !p.uiSchema?.["ui:widget"])) return p.children;
  const name = String(path[path.length - 1] ?? "");
  const since = sinceOf(p.schema);
  // A list entry is "Node ID 2", the list's singular and its position; rjsf's own "Node IDs-2 (required)" is for no one.
  const entry = typeof path[path.length - 1] === "number" ? (path[path.length - 1] as number) : null;
  const itemTitle = p.uiSchema?.["ui:itemTitle"];
  return (
    <FieldRow
      id={p.id}
      label={l?.label ?? (entry !== null && itemTitle ? `${itemTitle} ${entry + 1}` : p.label)}
      yamlKey={l?.label ? name : undefined}
      help={[l?.help ?? helpOf(p.rawDescription), since ? `Since schema ${since}.` : ""].filter(Boolean).join(" ") || undefined}
      changed={changedCount(ctx, path) > 0}
      required={p.required && entry === null}
      render={() => p.children}
    />
  );
}

/** One entry of a map (e.g. a pool's weights): the key is editable, the entry removable. */
function MapEntry(p: FieldTemplateProps) {
  const key = String(p.fieldPathId.path[p.fieldPathId.path.length - 1] ?? "");
  return (
    <div role="group" aria-label={`Entry ${key}`} className="flex items-start gap-2 py-1">
      <Input
        aria-label={`Key of entry ${key}`}
        mono
        defaultValue={key}
        disabled={p.disabled || p.readonly}
        onBlur={p.onKeyRenameBlur}
        className="w-48"
      />
      <div className="flex-1">{p.children}</div>
      <Button variant="danger" size="sm" onClick={p.onRemoveProperty} disabled={p.disabled || p.readonly}>
        Remove {key}
      </Button>
    </div>
  );
}

function ObjectTemplate(p: ObjectFieldTemplateProps) {
  const ctx = p.registry.formContext as Ctx;
  const path = p.fieldPathId.path;
  const own = lay(ctx, path);
  const visible = p.properties
    .filter((x) => !x.hidden)
    .map((x, i) => ({ x, i, l: lay(ctx, [...path, x.name]) }))
    .filter(({ x, l }) => !(l?.advanced && !ctx.showAdvanced) && onPage(ctx, [...path, x.name]))
    .sort((a, b) => (a.l?.order ?? Infinity) - (b.l?.order ?? Infinity) || a.i - b.i);
  const flat = visible.filter(({ l }) => !l?.group);
  const groups = [...new Set(visible.filter(({ l }) => l?.group).map(({ l }) => l!.group!))];
  const body = (
    <>
      {flat.map(({ x }) => (
        <div key={x.name}>{x.content}</div>
      ))}
      {p.schema.additionalProperties && (
        <Button size="sm" className="mt-1" onClick={p.onAddProperty} disabled={p.disabled || p.readonly}>
          <Plus aria-hidden size={14} strokeWidth={1.5} /> Add entry
        </Button>
      )}
      {groups.map((g) => {
        const members = visible.filter(({ l }) => l?.group === g);
        const changed = members.reduce((n, { x }) => n + changedCount(ctx, [...path, x.name]), 0);
        return (
          <Section key={g} title={g} changed={changed}>
            {members.map(({ x }) => (
              <div key={x.name}>{x.content}</div>
            ))}
          </Section>
        );
      })}
    </>
  );
  // The root, a list item and a property the parent already shows as a named group draw no section of their own.
  if (path.length === 0 || typeof path[path.length - 1] === "number" || own?.group) return body;
  return (
    <Section title={own?.label ?? (p.title || String(path[path.length - 1]))} changed={changedCount(ctx, path)} advanced={own?.advanced}>
      {body}
    </Section>
  );
}

function ArrayTemplate(p: ArrayFieldTemplateProps) {
  const ctx = p.registry.formContext as Ctx;
  const abs = absolute(ctx, p.fieldPathId.path);
  const title = lay(ctx, p.fieldPathId.path)?.label ?? (p.title || String(abs[abs.length - 1] ?? "items"));
  return (
    <Section title={title} changed={changedCount(ctx, p.fieldPathId.path)}>
      {p.items}
      {p.canAdd && (
        <Button size="sm" className="mt-1" onClick={p.onAddClick}>
          <Plus aria-hidden size={14} strokeWidth={1.5} /> Add {singular(title)}
        </Button>
      )}
    </Section>
  );
}

const singular = (title: string) => title.replace(/s$/, "");

function ArrayItemTemplate(p: ArrayFieldItemTemplateProps) {
  const title = String(p.parentUiSchema?.["ui:itemTitle"] ?? "item");
  return (
    <div role="group" aria-label={`${title} ${p.index + 1}`} className="border-t border-line py-2 first:border-t-0">
      {p.children}
      {p.buttonsProps.hasRemove && (
        <Button variant="danger" size="sm" onClick={p.buttonsProps.onRemoveItem}>
          Remove {title} {p.index + 1}
        </Button>
      )}
    </div>
  );
}

function uiFor(root: any, node: any, path: Path, layout: Layout): UiSchema {
  const n = resolve(root, node);
  const ui: UiSchema = {};
  const l = layout.fields[patternOf(path)];
  if (l?.widget) ui["ui:widget"] = WIDGET_NAMES[l.widget];
  const options: Record<string, boolean | string> = {};
  if (l?.help || helpOf(n.description) || sinceOf(n)) options.help = true;
  if (n.type === "integer" || n.type === "number") {
    ui["ui:widget"] ??= "NumberWidget"; // rjsf's default for numbers is the text widget
    const unit = l?.unit ?? unitFor(String(path[path.length - 1] ?? ""), n);
    if (unit) options.unit = unit;
  }
  if (Object.keys(options).length) ui["ui:options"] = options;
  if (n.type === "object" && n.properties)
    for (const [k, v] of Object.entries<any>(n.properties)) ui[k] = uiFor(root, v, [...path, k], layout);
  else if (n.type === "array" && n.items) {
    const item = resolve(root, n.items);
    const itemTitle = singular(layout.fields[patternOf(path)]?.label ?? (n.title as string | undefined) ?? String(path[path.length - 1] ?? "item"));
    if (item.type === "string" && !item.enum) ui["ui:widget"] ??= "lines";
    else ui.items = { ...uiFor(root, n.items, [...path, 0], layout), "ui:itemTitle": itemTitle };
    ui["ui:itemTitle"] = itemTitle;
  }
  return ui;
}

export interface SchemaFormProps {
  /** The configuration schema as the controller serves it. */
  schema: Record<string, any>;
  /** The whole document value; `onChange` returns the whole value with the edit applied. */
  value: unknown;
  /** The saved document value, for the changed marks; defaults to `value` (nothing marked). */
  saved?: unknown;
  onChange: (value: unknown) => void;
  layout?: Layout;
  /** The part of the document to render, e.g. `["settings"]` or `["pools"]`; default is the whole document. */
  path?: Path;
  showAdvanced?: boolean;
  /** Only fields (and parents of fields) the layout puts on this page. */
  page?: string;
  /** Every field is shown but cannot be changed (a role that cannot save). */
  readonly?: boolean;
  /** Only these top-level fields of the rendered object (a source type takes some fields and not others). */
  only?: string[];
}

/**
 * Renders a part of the configuration as a form from its JSON Schema (rjsf for `$ref`, `enum` and list handling,
 * our components for everything you see). The layout groups, orders and explains; the YAML never changes meaning.
 */
export function SchemaForm({ schema, value, saved, onChange, layout = DEFAULT_LAYOUT, path = [], showAdvanced = false, page, readonly = false, only }: SchemaFormProps) {
  const root = useMemo(() => simplify(schema) as Record<string, any>, [schema]);
  const sub = useMemo(() => {
    const s = subschema(root, path);
    return s && ({ ...s, $defs: root.$defs } as RJSFSchema);
  }, [root, path.join("\u0000")]); // eslint-disable-line react-hooks/exhaustive-deps
  const ui = useMemo(() => (sub ? uiFor(root, sub, path, layout) : {}), [root, sub, layout, path.join("\u0000")]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!sub) return <p role="alert" className="text-sm text-bad">The schema has no part at {path.join(".") || "the root"}.</p>;

  const ctx: Ctx = { layout, saved: saved ?? value, current: value, root: path, showAdvanced, page, only };
  return (
    <Form
      schema={sub}
      uiSchema={ui}
      validator={validator}
      formData={get(value, path)}
      formContext={ctx}
      widgets={WIDGETS}
      templates={{
        FieldTemplate,
        ObjectFieldTemplate: ObjectTemplate,
        ArrayFieldTemplate: ArrayTemplate,
        ArrayFieldItemTemplate: ArrayItemTemplate,
        ButtonTemplates: { SubmitButton: () => null },
      }}
      tagName="div"
      readonly={readonly}
      noHtml5Validate
      omitExtraData={false}
      onChange={({ formData }) => {
        const next = set(value, path, coerce(root, sub, formData));
        if (JSON.stringify(next) !== JSON.stringify(value)) onChange(next);
      }}
    />
  );
}
