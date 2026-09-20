"use client";

import * as React from "react";
import { useI18n } from "@/lib/i18n";
import { Field, Input } from "@/components/ui/field";

/**
 * A number field that never invents a value.
 *
 * The console used to write `Number(value) || 0` into the draft, so clearing the box silently
 * meant zero. That is not what the router would use: the parser clamps several of these fields
 * (connections and timeouts to at least 1, the refresh/skip intervals to at least 5, thresholds to
 * at least 1), so the form showed a number that could never take effect. This component keeps the
 * two ends honest by taking the same lower bound the parser uses and showing an error - and
 * blocking Save - while the box is empty or below it.
 *
 * The bound is passed in from the caller rather than defined here, because src/config.rs is the
 * only authority on it; duplicating a "reasonable" minimum in the UI is how the two drift apart.
 */
export function NumberField({
  label,
  help,
  hint,
  value,
  min,
  max,
  step,
  unit,
  onChange,
}: {
  label: React.ReactNode;
  help?: React.ReactNode;
  hint?: React.ReactNode;
  value: number;
  /** Smallest value the parser accepts. Values below it are silently raised, so the UI rejects them. */
  min: number;
  max?: number;
  step?: number;
  /** Shown after the error, e.g. "seconds". */
  unit?: string;
  onChange: (v: number) => void;
}) {
  const { t } = useI18n();
  // The text is the source of truth while typing: parsing on every keystroke would fight the user
  // ("1" on the way to "10" is not an error worth showing) and would rewrite the box under them.
  const [text, setText] = React.useState(String(value));
  const [focused, setFocused] = React.useState(false);

  React.useEffect(() => {
    if (!focused) setText(String(value));
  }, [value, focused]);

  const num = text.trim() === "" ? Number.NaN : Number(text);
  const tooLow = !Number.isNaN(num) && num < min;
  const tooHigh = max !== undefined && !Number.isNaN(num) && num > max;
  const invalid = Number.isNaN(num) || tooLow || tooHigh;

  const message = Number.isNaN(num)
    ? t.field.required
    : tooLow
      ? t.field.minValue.replace("{min}", String(min)).replace("{unit}", unit ? " " + unit : "")
      : tooHigh
        ? t.field.maxValue.replace("{max}", String(max)).replace("{unit}", unit ? " " + unit : "")
        : null;

  return (
    <Field label={label} help={help} hint={hint} error={message}>
      <Input
        type="number"
        min={min}
        max={max}
        step={step}
        value={text}
        aria-invalid={invalid || undefined}
        onFocus={() => setFocused(true)}
        onBlur={() => {
          setFocused(false);
          // An empty or out-of-range box is not silently repaired: it keeps what was typed so the
          // error stays visible, and the draft keeps the last valid value.
          if (!invalid) onChange(num);
        }}
        onChange={(e) => {
          const next = e.target.value;
          setText(next);
          const n = Number(next);
          if (next.trim() !== "" && !Number.isNaN(n)) onChange(n);
        }}
      />
    </Field>
  );
}
