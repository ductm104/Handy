import React from "react";
import { useTranslation } from "react-i18next";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import type { TranscriptionBreakMode } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";

interface TranscriptionBreakModeProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const TranscriptionBreakModeSetting: React.FC<TranscriptionBreakModeProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const options = [
      {
        value: "none",
        label: t("settings.advanced.transcriptionBreakMode.options.none"),
      },
      {
        value: "sentence",
        label: t("settings.advanced.transcriptionBreakMode.options.sentence"),
      },
      {
        value: "word",
        label: t("settings.advanced.transcriptionBreakMode.options.word"),
      },
    ];

    const selectedMode = (getSetting("transcription_break_mode") ||
      "none") as string;

    return (
      <SettingContainer
        title={t("settings.advanced.transcriptionBreakMode.title")}
        description={t("settings.advanced.transcriptionBreakMode.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      >
        <Dropdown
          options={options}
          selectedValue={selectedMode}
          onSelect={(value) =>
            updateSetting(
              "transcription_break_mode",
              value as TranscriptionBreakMode,
            )
          }
          disabled={isUpdating("transcription_break_mode")}
        />
      </SettingContainer>
    );
  });
