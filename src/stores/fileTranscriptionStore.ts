import { create } from "zustand";
import { commands, type FileTranscriptionResult } from "@/bindings";

type TranscriptionResponse =
  | { status: "ok"; data: FileTranscriptionResult }
  | { status: "busy" }
  | { status: "no-file" }
  | { status: "error"; error: string };

interface FileTranscriptionStore {
  selectedPath: string | null;
  result: FileTranscriptionResult | null;
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
    isTranscribing: false,
    error: null,

    setSelectedPath: (selectedPath) => {
      if (get().isTranscribing) return;
      set({ selectedPath, result: null, error: null });
    },

    clearFile: () => {
      if (get().isTranscribing) return;
      set({ selectedPath: null, result: null, error: null });
    },

    transcribeSelectedFile: async (postProcess) => {
      const { selectedPath, isTranscribing } = get();

      if (isTranscribing) {
        return { status: "busy" };
      }

      if (!selectedPath) {
        return { status: "no-file" };
      }

      set({ isTranscribing: true, error: null });

      try {
        const response = await commands.transcribeFile(
          selectedPath,
          postProcess,
        );

        if (response.status === "error") {
          set({ error: response.error });
          return response;
        }

        set({ result: response.data, error: null });
        return response;
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        set({ error: message });
        return { status: "error", error: message };
      } finally {
        set({ isTranscribing: false });
      }
    },
  }),
);
