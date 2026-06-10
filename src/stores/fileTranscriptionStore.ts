import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import { commands, type FileTranscriptionResult } from "@/bindings";

type FileTranscriptionProgressStage =
  | "loading_model"
  | "decoding"
  | "transcribing"
  | "post_processing"
  | "complete";

interface FileTranscriptionProgressEvent {
  stage: FileTranscriptionProgressStage;
  text: string | null;
  progress: number | null;
}

type TranscriptionResponse =
  | { status: "ok"; data: FileTranscriptionResult }
  | { status: "busy" }
  | { status: "no-file" }
  | { status: "cancelled" }
  | { status: "error"; error: string };

interface FileTranscriptionStore {
  selectedPath: string | null;
  result: FileTranscriptionResult | null;
  partialText: string;
  progress: number | null;
  progressStage: FileTranscriptionProgressStage | null;
  isTranscribing: boolean;
  isStopping: boolean;
  error: string | null;
  setSelectedPath: (path: string) => void;
  clearFile: () => void;
  transcribeSelectedFile: (
    postProcess: boolean,
  ) => Promise<TranscriptionResponse>;
  cancelTranscription: () => void;
}

export const useFileTranscriptionStore = create<FileTranscriptionStore>()(
  (set, get) => ({
    selectedPath: null,
    result: null,
    partialText: "",
    progress: null,
    progressStage: null,
    isTranscribing: false,
    isStopping: false,
    error: null,

    setSelectedPath: (selectedPath) => {
      if (get().isTranscribing) return;
      set({
        selectedPath,
        result: null,
        partialText: "",
        progress: null,
        progressStage: null,
        error: null,
      });
    },

    clearFile: () => {
      if (get().isTranscribing) return;
      set({
        selectedPath: null,
        result: null,
        partialText: "",
        progress: null,
        progressStage: null,
        error: null,
      });
    },

    transcribeSelectedFile: async (postProcess) => {
      const { selectedPath, isTranscribing } = get();

      if (isTranscribing) {
        return { status: "busy" };
      }

      if (!selectedPath) {
        return { status: "no-file" };
      }

      set({
        isTranscribing: true,
        isStopping: false,
        error: null,
        result: null,
        partialText: "",
        progress: null,
        progressStage: "loading_model",
      });

      await new Promise((resolve) => setTimeout(resolve, 0));

      let unlistenProgress: (() => void) | null = null;

      try {
        unlistenProgress = await listen<FileTranscriptionProgressEvent>(
          "file-transcription-progress",
          (event) => {
            const { stage, text, progress } = event.payload;

            set({
              progressStage: stage,
              progress,
              ...(text !== null ? { partialText: text } : {}),
            });
          },
        );

        const response = await commands.transcribeFile(
          selectedPath,
          postProcess,
        );

        if (response.status === "error") {
          if (response.error === "Cancelled") {
            return { status: "cancelled" };
          }
          set({ error: response.error });
          return response;
        }

        set({ result: response.data, partialText: "", error: null });
        return response;
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        set({ error: message });
        return { status: "error", error: message };
      } finally {
        unlistenProgress?.();
        set({
          isTranscribing: false,
          isStopping: false,
          partialText: "",
          progress: null,
          progressStage: null,
        });
      }
    },

    cancelTranscription: () => {
      set({ isStopping: true });
      commands.cancelFileTranscription();
    },
  }),
);
