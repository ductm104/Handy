import React from "react";
import { useTranslation } from "react-i18next";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { FileTranscriptionPanel } from "../FileTranscriptionPanel";
import { ModelSettingsCard } from "./ModelSettingsCard";

export const GeneralSettings: React.FC = () => {
  const { t } = useTranslation();

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup
        title={t("settings.fileTranscription.title")}
        description={t("settings.fileTranscription.description")}
      >
        <FileTranscriptionPanel />
      </SettingsGroup>
      <ModelSettingsCard />
    </div>
  );
};
