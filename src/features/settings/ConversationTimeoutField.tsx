import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { Field } from "./SettingsFields";
import {
  conversationTimeoutMsFromSecondsInput,
  conversationTimeoutSecondsInputValue,
  LEGACY_DYNAMIC_LAN_MAX_REQUEST_TIMEOUT_MS,
  MAX_CONVERSATION_TIMEOUT_SECONDS,
  MIN_CONVERSATION_TIMEOUT_SECONDS,
} from "../../lib/conversationTimeout";
export function ConversationTimeoutField({
  timeoutMs,
  legacyDynamicLan,
  onValidityChange,
  onChange,
}: {
  timeoutMs: number;
  legacyDynamicLan: boolean;
  onValidityChange: (valid: boolean) => void;
  onChange: (timeoutMs: number) => void;
}) {
  const { t } = useTranslation();
  const canonicalValue = conversationTimeoutSecondsInputValue(timeoutMs);
  const [inputValue, setInputValue] = useState(canonicalValue);
  const parsedTimeoutMs = conversationTimeoutMsFromSecondsInput(inputValue);
  const invalid = parsedTimeoutMs === null;
  const legacyLimitExceeded =
    legacyDynamicLan && timeoutMs > LEGACY_DYNAMIC_LAN_MAX_REQUEST_TIMEOUT_MS;
  const fieldInvalid = invalid;

  useEffect(() => setInputValue(canonicalValue), [canonicalValue]);
  useEffect(() => onValidityChange(!fieldInvalid), [fieldInvalid, onValidityChange]);
  useEffect(() => () => onValidityChange(true), [onValidityChange]);

  return (
    <Field label={t("settings.connection.llmTimeoutSeconds")}>
      <input
        type="number"
        min={MIN_CONVERSATION_TIMEOUT_SECONDS}
        max={MAX_CONVERSATION_TIMEOUT_SECONDS}
        step={0.001}
        value={inputValue}
        aria-describedby="llm-timeout-seconds-help"
        aria-invalid={fieldInvalid}
        onChange={(event) => {
          const next = event.currentTarget.value;
          setInputValue(next);
          const nextTimeoutMs = conversationTimeoutMsFromSecondsInput(next);
          if (nextTimeoutMs !== null && nextTimeoutMs !== timeoutMs) onChange(nextTimeoutMs);
        }}
        onBlur={() =>
          setInputValue(
            parsedTimeoutMs === null
              ? canonicalValue
              : conversationTimeoutSecondsInputValue(parsedTimeoutMs),
          )
        }
        onKeyDown={(event) => {
          if (event.key === "Enter") event.currentTarget.blur();
          if (event.key === "Escape") setInputValue(canonicalValue);
        }}
      />
      <small
        id="llm-timeout-seconds-help"
        className={fieldInvalid ? "settings-field-hint error" : "settings-field-hint"}
      >
        {t(
          invalid
            ? "settings.connection.llmTimeoutInvalid"
            : legacyLimitExceeded
              ? "settings.connection.llmTimeoutLegacyLimit"
              : "settings.connection.llmTimeoutHint",
        )}
      </small>
    </Field>
  );
}
