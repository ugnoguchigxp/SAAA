import { invoke } from "@tauri-apps/api/core";
import { LarmVoiceOwners } from "./larmVoiceOwner";
const owners = new LarmVoiceOwners(
  (name, args) => invoke<void>(name, args),
  () => crypto.randomUUID(),
);
export const ownLarmVoice = owners.own.bind(owners);
export const currentLarmVoice = owners.current.bind(owners);
export const endLarmVoice = owners.end.bind(owners);
export const prepareLarmVoiceSession = owners.prepare.bind(owners);
export const failLarmVoiceSession = owners.fail.bind(owners);
