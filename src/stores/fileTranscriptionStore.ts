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
  | { status: "error"; error: string };

interface FileTranscriptionStore {
  selectedPath: string | null;
  result: FileTranscriptionResult | null;
  partialText: string;
  progress: number | null;
  progressStage: FileTranscriptionProgressStage | null;
  isTranscribing: boolean;
  error: string | null;
  setSelectedPath: (path: string) => void;
  clearFile: () => void;
  transcribeSelectedFile: (
    postProcess: boolean,
  ) => Promise<TranscriptionResponse>;
}

export const useFileTranscriptionStore = create<FileTranscriptionStore>()(
  (set, get) => ({
    selectedPath: null,
    result: null,
    partialText: "",
    progress: null,
    progressStage: null,
    isTranscribing: false,
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
        error: null,
        result: null,
        partialText: "",
        progress: null,
        progressStage: "loading_model",
      });

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
          partialText: "",
          progress: null,
          progressStage: null,
        });
      }
    },
  }),
);
