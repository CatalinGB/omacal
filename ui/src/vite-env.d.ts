/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** `"web"` for the omacal-server build; unset for the desktop build. */
  readonly VITE_TARGET?: string;
}
