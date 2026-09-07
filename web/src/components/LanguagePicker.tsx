import { Select } from "./ui";
import * as Icon from "./icons";
import { Tooltip } from "./Tooltip";
import { LANGUAGES, useI18n, type Language } from "../i18n";

/**
 * Switches the active language and remembers the choice.
 *
 * Wide screens get a labelled dropdown; narrow screens get a globe icon
 * whose invisible native select keeps it usable with touch and keyboard.
 * Reused by the header and by the signed-out pages (login, setup, recovery)
 * so the language can always be changed in place.
 */
export function LanguagePicker() {
  const { language, setLanguage, t } = useI18n();
  const options = () =>
    Object.entries(LANGUAGES).map(([code, { label }]) => (
      <option key={code} value={code}>
        {label}
      </option>
    ));

  return (
    <>
      <Select
        value={language}
        aria-label={t("nav.language")}
        onChange={(e) => setLanguage((e.target as HTMLSelectElement).value as Language)}
        class="hidden !w-auto !py-1.5 !text-xs lg:block"
      >
        {options()}
      </Select>

      <Tooltip label={t("nav.language")}>
        <span class="group relative flex size-9 items-center justify-center rounded-lg text-fg-muted transition-colors hover:bg-ink-700 hover:text-fg focus-within:bg-ink-700 focus-within:text-fg lg:hidden">
          <Icon.Globe size={16} />
          <select
            value={language}
            aria-label={t("nav.language")}
            title={t("nav.language")}
            onChange={(e) => setLanguage((e.target as HTMLSelectElement).value as Language)}
            class="absolute inset-0 h-full w-full cursor-pointer appearance-none opacity-0 focus:outline-none"
          >
            {options()}
          </select>
        </span>
      </Tooltip>
    </>
  );
}
