"use client";

import * as React from "react";
import { cn } from "@/lib/utils";
import { useI18n } from "@/lib/i18n";

/**
 * Every editable setting goes through this: label + inline help. The help text is
 * not optional decoration — the whole point is that an operator never has to leave
 * the page to find out what a knob does.
 */

/// Set by a Field whose child is a control *group* rather than one labelable element.
/// A <label for> may only target a labelable element, so those Field labels drop htmlFor and
/// the group names itself with aria-labelledby instead.
const GroupFieldContext = React.createContext<{ mark: () => void } | undefined>(undefined);

export function Field({
  label,
  help,
  hint,
  required,
  error,
  children,
  className,
  htmlFor,
}: {
  label: React.ReactNode;
  help?: React.ReactNode;
  hint?: React.ReactNode;
  required?: boolean;
  error?: string | null;
  children: React.ReactNode;
  className?: string;
  htmlFor?: string;
}) {
  const { t } = useI18n();
  const [open, setOpen] = React.useState(false);
  // A generated id keeps the label/control association intact even when the caller does not
  // pass one - screen readers and the browser's own autofill both rely on it.
  const auto = React.useId();
  const id = htmlFor ?? auto;
  // A chip list is invoked as a plain function, so it cannot be recognised from `children`.
  // It reports itself through this registration during the same render, which flips the label
  // from "points at a control" to "names the group".
  const [isGroup, setIsGroup] = React.useState(false);
  const groupRegistration = React.useMemo(
    () => ({ mark: () => setIsGroup((prev) => (prev ? prev : true)) }),
    [],
  );
  return (
    <div className={cn("min-w-0", className)}>
      <div className="flex items-baseline justify-between gap-2">
        <label
          htmlFor={isGroup ? undefined : id}
          id={`${id}-label`}
          className="mono text-xs uppercase tracking-[0.12em] text-[var(--ink-dim)]"
        >
          {label}
          {required ? <span className="ml-1 text-[var(--signal)]">*</span> : null}
        </label>
        {help ? (
          <button
            type="button"
            onClick={() => setOpen((v) => !v)}
            aria-expanded={open}
            aria-label={t.field.help}
            className={cn(
              "mono flex h-4 w-4 shrink-0 items-center justify-center rounded-full border text-2xs leading-none transition-colors",
              open
                ? "border-[var(--signal)] text-[var(--signal)]"
                : "border-[var(--line-strong)] text-[var(--ink-faint)] hover:border-[var(--signal)] hover:text-[var(--signal)]",
            )}
          >
            ?
          </button>
        ) : null}
      </div>
      <div className="mt-1.5">
        <FieldIdContext.Provider value={{ id, labelId: `${id}-label` }}>
          <GroupFieldContext.Provider value={groupRegistration}>{children}</GroupFieldContext.Provider>
        </FieldIdContext.Provider>
      </div>
      {open && help ? (
        <div className="rise mt-1.5 border-l-2 border-[var(--signal)]/50 bg-[var(--panel-2)] px-2.5 py-2 text-xs leading-relaxed text-[var(--ink-dim)]">
          {help}
        </div>
      ) : null}
      {!open && hint ? <div className="mt-1 text-xs leading-snug text-[var(--ink-faint)]">{hint}</div> : null}
      {error ? <div className="mt-1 text-xs text-[var(--danger)]">{error}</div> : null}
    </div>
  );
}

/// Carries the Field's generated ids down to the control(s) it wraps, so a control is always
/// bound to its label (WCAG 4.1.2 / 3.3.2) without every call site passing an id.
const FieldIdContext = React.createContext<{ id: string; labelId: string } | undefined>(undefined);

function useFieldIds(explicit?: string): { id: string | undefined; labelId: string | undefined } {
  const inherited = React.useContext(FieldIdContext);
  const generated = React.useId();
  return {
    id: explicit ?? inherited?.id ?? generated,
    labelId: inherited?.labelId,
  };
}

function useFieldId(explicit?: string): string | undefined {
  return useFieldIds(explicit).id;
}

const CONTROL =
  "mono w-full rounded-[2px] border border-[var(--line-strong)] bg-[var(--panel-2)] px-2.5 text-sm text-[var(--ink)] " +
  "placeholder:text-[var(--ink-faint)] transition-colors focus:border-[var(--signal)] focus:outline-none " +
  "disabled:opacity-50";

export const Input = React.forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement>>(
  function Input({ className, id, ...rest }, ref) {
    const fieldId = useFieldId(id);
    return <input ref={ref} id={fieldId} name={rest.name ?? fieldId} className={cn(CONTROL, "h-9", className)} {...rest} />;
  },
);

export const Textarea = React.forwardRef<HTMLTextAreaElement, React.TextareaHTMLAttributes<HTMLTextAreaElement>>(
  function Textarea({ className, id, ...rest }, ref) {
    const fieldId = useFieldId(id);
    return <textarea ref={ref} id={fieldId} name={rest.name ?? fieldId} className={cn(CONTROL, "py-2 leading-relaxed", className)} {...rest} />;
  },
);

export const Select = React.forwardRef<HTMLSelectElement, React.SelectHTMLAttributes<HTMLSelectElement>>(
  function Select({ className, children, id, ...rest }, ref) {
    const fieldId = useFieldId(id);
    return (
      <select
        ref={ref}
        id={fieldId}
        name={rest.name ?? fieldId}
        className={cn(CONTROL, "h-9 appearance-none pr-7", className)}
        {...rest}
      >
        {children}
      </select>
    );
  },
);

export function Switch({
  checked,
  onChange,
  label,
  id,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  /** Accessible name; a bare switch is meaningless to a screen reader without one. */
  label?: string;
  id?: string;
  disabled?: boolean;
}) {
  return (
    <button
      id={id}
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label ?? "toggle"}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        "relative inline-flex h-[22px] w-[42px] shrink-0 items-center rounded-[2px] border transition-colors duration-200 disabled:opacity-40",
        checked ? "border-[var(--up)]/60 bg-[var(--up)]/18" : "border-[var(--line-strong)] bg-[var(--panel-2)]",
      )}
    >
      <span
        className={cn(
          "ml-[2px] h-[16px] w-[16px] rounded-[1px] transition-transform duration-200",
          checked ? "translate-x-[20px]" : "translate-x-0",
        )}
        style={{ background: checked ? "var(--up)" : "var(--ink-faint)" }}
      />
    </button>
  );
}

export function ToggleRow({
  label,
  help,
  checked,
  onChange,
  disabled,
}: {
  label: React.ReactNode;
  help?: React.ReactNode;
  checked: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <div className="flex items-start justify-between gap-4 py-2">
      <Field label={label} help={help} className="flex-1">
        <span />
      </Field>
      <div className="pt-[2px]">
        <Switch checked={checked} onChange={onChange} label={typeof label === "string" ? label : undefined} disabled={disabled} />
      </div>
    </div>
  );
}

/** Chips for list-of-string settings (session headers, dropped params, ...). */
export function ChipInput({
  values,
  onChange,
  placeholder,
  disabled,
}: {
  values: string[];
  onChange: (v: string[]) => void;
  placeholder?: string;
  disabled?: boolean;
}) {
  const [draft, setDraft] = React.useState("");
  // A chip list is a group of controls, so a <label for> is illegal here (it may only target a
  // labelable element). The group is named with aria-labelledby pointing at the Field label,
  // and the Field is told to drop its htmlFor.
  const { id: fieldId, labelId } = useFieldIds();
  React.useContext(GroupFieldContext)?.mark();
  const add = () => {
    const v = draft.trim();
    if (!v) return;
    if (!values.includes(v)) onChange([...values, v]);
    setDraft("");
  };
  return (
    <div
      data-control-group="true"
      className="rounded-[2px] border border-[var(--line-strong)] bg-[var(--panel-2)] p-1.5"
    >
      <div
        id={fieldId}
        role="group"
        aria-labelledby={labelId}
        aria-label={labelId ? undefined : (placeholder ?? "values")}
        className="flex flex-wrap items-center gap-1.5"
      >
        {values.map((v) => (
          <span
            key={v}
            className="mono inline-flex items-center gap-1 rounded-[2px] border border-[var(--line-strong)] bg-[var(--panel)] px-1.5 py-[3px] text-xs"
          >
            {v}
            <button
              type="button"
              disabled={disabled}
              onClick={() => onChange(values.filter((x) => x !== v))}
              className="text-[var(--ink-faint)] transition-colors hover:text-[var(--danger)] disabled:opacity-40"
              aria-label={`remove ${v}`}
            >
              ×
            </button>
          </span>
        ))}
        <input
          value={draft}
          disabled={disabled}
          // Not tied to the Field's label: this is a secondary "add one more" control, so it
          // carries its own accessible name and stays out of the parent label association.
          name="chip-value"
          aria-label={placeholder ?? "add value"}
          placeholder={placeholder}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={add}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === ",") {
              e.preventDefault();
              add();
            } else if (e.key === "Backspace" && !draft && values.length) {
              onChange(values.slice(0, -1));
            }
          }}
          className="mono min-w-[120px] flex-1 bg-transparent px-1 py-[3px] text-xs outline-none placeholder:text-[var(--ink-faint)]"
        />
      </div>
    </div>
  );
}
