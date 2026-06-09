import React, { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import { Check, Clipboard, FileAudio, Loader2, Upload, X } from "lucide-react";
import { toast } from "sonner";
import { Button } from "../ui/Button";
import { Textarea } from "../ui/Textarea";
import { useSettings } from "@/hooks/useSettings";
import { useFileTranscriptionStore } from "@/stores/fileTranscriptionStore";
import { useModelStore } from "@/stores/modelStore";

const MEDIA_EXTENSIONS = [
  "mp3",
  "mp4",
  "m4a",
  "aac",
  "wav",
  "flac",
  "ogg",
  "oga",
];

const getFileName = (path: string) => path.split(/[\\/]/).pop() || path;

export const FileTranscriptionPanel: React.FC = () => {
  const { t } = useTranslation();
  const { settings } = useSettings();
  const { currentModel, models } = useModelStore();
  const {
    selectedPath,
    result,
    partialText,
    progress,
    progressStage,
    isTranscribing,
    setSelectedPath,
    clearFile,
    transcribeSelectedFile,
  } = useFileTranscriptionStore();
  const [copied, setCopied] = useState(false);

  const selectedFileName = useMemo(
    () => (selectedPath ? getFileName(selectedPath) : null),
    [selectedPath],
  );

  const currentModelInfo = models.find((model) => model.id === currentModel);
  const canTranscribe =
    Boolean(selectedPath) &&
    Boolean(currentModelInfo?.is_downloaded) &&
    !isTranscribing;
  const outputText = isTranscribing ? partialText : (result?.text ?? "");

  const getProgressLabel = () => {
    switch (progressStage) {
      case "loading_model":
        return t("settings.fileTranscription.progress.loadingModel");
      case "decoding":
        return t("settings.fileTranscription.progress.decoding");
      case "post_processing":
        return t("settings.fileTranscription.progress.postProcessing");
      case "complete":
        return t("settings.fileTranscription.progress.complete");
      case "transcribing":
      default:
        return t("settings.fileTranscription.progress.transcribing");
    }
  };

  const progressLabel =
    isTranscribing && progress !== null
      ? t("settings.fileTranscription.progress.withPercentage", {
          label: getProgressLabel(),
          progress,
        })
      : getProgressLabel();

  const handleSelectFile = async () => {
    const selected = await open({
      multiple: false,
      filters: [
        {
          name: t("settings.fileTranscription.fileFilterName"),
          extensions: MEDIA_EXTENSIONS,
        },
      ],
    });

    if (typeof selected !== "string") return;

    setSelectedPath(selected);
    setCopied(false);
  };

  const handleClearFile = () => {
    clearFile();
    setCopied(false);
  };

  const handleTranscribe = async () => {
    if (!selectedPath) {
      toast.error(t("settings.fileTranscription.errors.noFile"));
      return;
    }

    if (!currentModelInfo?.is_downloaded) {
      toast.error(t("settings.fileTranscription.errors.noModel"));
      return;
    }

    setCopied(false);

    const response = await transcribeSelectedFile(
      settings?.post_process_enabled ?? false,
    );

    if (response.status === "ok") {
      toast.success(t("settings.fileTranscription.success"));
      return;
    }

    if (response.status === "busy") {
      return;
    }

    if (response.status === "no-file") {
      toast.error(t("settings.fileTranscription.errors.noFile"));
      return;
    }

    console.error("Failed to transcribe file:", response.error);
    toast.error(t("settings.fileTranscription.errors.transcribe"), {
      description: response.error,
    });
  };

  const handleCopy = async () => {
    if (!result?.text) return;

    try {
      await navigator.clipboard.writeText(result.text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch (error) {
      console.error("Failed to copy transcription:", error);
      toast.error(t("settings.fileTranscription.errors.copy"));
    }
  };

  return (
    <div className="p-4 space-y-4">
      <button
        type="button"
        onClick={handleSelectFile}
        disabled={isTranscribing}
        className="w-full min-h-36 rounded-lg border border-dashed border-mid-gray/60 bg-mid-gray/5 px-4 py-5 text-start transition-colors hover:border-logo-primary hover:bg-logo-primary/10 focus:outline-none focus:border-logo-primary disabled:opacity-60 disabled:cursor-not-allowed"
      >
        <div className="flex items-start gap-3">
          <div className="mt-0.5 flex h-10 w-10 shrink-0 items-center justify-center rounded-md bg-logo-primary/15 text-logo-primary">
            {selectedFileName ? <FileAudio size={22} /> : <Upload size={22} />}
          </div>
          <div className="min-w-0 space-y-1">
            <div className="text-sm font-semibold">
              {selectedFileName ?? t("settings.fileTranscription.selectTitle")}
            </div>
            <div className="text-xs text-mid-gray break-words">
              {selectedFileName
                ? selectedPath
                : t("settings.fileTranscription.selectDescription")}
            </div>
            <div className="text-xs text-mid-gray">
              {t("settings.fileTranscription.supportedFormats")}
            </div>
          </div>
        </div>
      </button>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          type="button"
          variant="secondary"
          onClick={handleSelectFile}
          disabled={isTranscribing}
        >
          {selectedPath
            ? t("settings.fileTranscription.changeFile")
            : t("settings.fileTranscription.selectFile")}
        </Button>
        {selectedPath && (
          <Button
            type="button"
            variant="ghost"
            onClick={handleClearFile}
            disabled={isTranscribing}
            title={t("settings.fileTranscription.removeFile")}
          >
            <X size={16} />
            {t("settings.fileTranscription.removeFile")}
          </Button>
        )}
        <Button
          type="button"
          onClick={handleTranscribe}
          disabled={!canTranscribe}
          className="ms-auto"
        >
          {isTranscribing && (
            <Loader2 size={16} className="me-2 animate-spin" />
          )}
          {isTranscribing
            ? t("settings.fileTranscription.transcribing")
            : t("settings.fileTranscription.transcribe")}
        </Button>
      </div>

      {!currentModelInfo?.is_downloaded && (
        <div className="rounded-md border border-mid-gray/20 bg-mid-gray/5 px-3 py-2 text-xs text-mid-gray">
          {t("settings.fileTranscription.noModel")}
        </div>
      )}

      <div className="space-y-2">
        <div className="flex items-center justify-between gap-2">
          <h3 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("settings.fileTranscription.outputTitle")}
          </h3>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={handleCopy}
            disabled={isTranscribing || !result?.text}
          >
            {copied ? (
              <Check size={14} className="me-1" />
            ) : (
              <Clipboard size={14} className="me-1" />
            )}
            {copied
              ? t("settings.fileTranscription.copied")
              : t("settings.fileTranscription.copy")}
          </Button>
        </div>
        {isTranscribing && (
          <div className="text-xs text-mid-gray">{progressLabel}</div>
        )}
        <Textarea
          readOnly
          value={outputText}
          placeholder={
            isTranscribing
              ? progressLabel
              : t("settings.fileTranscription.outputPlaceholder")
          }
          className="w-full min-h-44 select-text font-normal"
        />
      </div>
    </div>
  );
};
