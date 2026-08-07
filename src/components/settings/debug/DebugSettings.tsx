import React from "react";
import { useTranslation } from "react-i18next";
import { WordCorrectionThreshold } from "./WordCorrectionThreshold";
import { LogLevelSelector } from "./LogLevelSelector";
import { PasteDelay } from "./PasteDelay";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { UpdateChecksToggle } from "../UpdateChecksToggle";

export const DebugSettings: React.FC = () => {
  const { t } = useTranslation();

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.debug.title")}>
        <LogLevelSelector grouped={true} />
        <UpdateChecksToggle descriptionMode="tooltip" grouped={true} />
        <WordCorrectionThreshold descriptionMode="tooltip" grouped={true} />
        <PasteDelay descriptionMode="tooltip" grouped={true} />
      </SettingsGroup>
    </div>
  );
};
