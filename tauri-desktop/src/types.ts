// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-UBW4VJOMLXIG
// types.ts - Shared TypeScript interfaces matching Rust backend
// Copyright 2025 Sassy Consulting LLC. All rights reserved.

export interface PeerInfo {
  device_id: number;
  device_name: string;
  address: string;
  last_seen: number;
  channel: number;
  public_key?: string;
  key_exchanged?: boolean;
}

export type ConnectionStatusType =
  | 'Disconnected'
  | 'Discovering'
  | 'Connected'
  | 'Transmitting'
  | 'Receiving';

export interface AppStatus {
  connection_status: ConnectionStatusType;
  channel: number;
  peer_count: number;
  is_transmitting: boolean;
  is_receiving: boolean;
}

export interface DeviceInfo {
  device_id: string;
  device_name: string;
  version: string;
}

export interface Volume {
  input: number;
  output: number;
}

export interface AudioDeviceInfo {
  name: string;
  is_default: boolean;
  device_type: string;
}

export interface AudioDevices {
  inputs: AudioDeviceInfo[];
  outputs: AudioDeviceInfo[];
}

export interface NetworkInfo {
  port: number;
  multicast_addr: string;
  use_random_port: boolean;
  encryption_enabled: boolean;
  is_encrypted: boolean;
  public_key: string | null;
}

// Mirrors CellularTransport::stats_json() in transport/cellular.rs.
// state: "disconnected" | "connecting" | "connected" | "error".
export interface CellularStatus {
  state: string;
  room: string;
  sent: number;
  received: number;
}

export type View = 'lobby' | 'walkie' | 'settings';
