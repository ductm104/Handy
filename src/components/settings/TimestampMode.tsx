import React from "react";
import { useTranslation } from "react-i18next";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";
import type { TimestampMode } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";

interface TimestampModeProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const TimestampModeSetting: React.FC<TimestampModeProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const options = [
      {
        value: "plain",
        label: t("settings.advanced.timestampMode.options.plain"),
      },
      {
        value: "timestamp",
        label: t("settings.advanced.timestampMode.options.timestamp"),
      },
    ];

    const selectedMode = (getSetting("timestamp_mode") || "plain") as string;

    return (
      <SettingContainer
        title={t("settings.advanced.timestampMode.title")}
        description={t("settings.advanced.timestampMode.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
        tooltipPosition="bottom"
      >
        <Dropdown
          options={options}
          selectedValue={selectedMode}
          onSelect={(value) =>
            updateSetting("timestamp_mode", value as TimestampMode)
          }
          disabled={isUpdating("timestamp_mode")}
        />
      </SettingContainer>
    );
  },
);
