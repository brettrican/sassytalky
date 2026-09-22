// Copyright (c) 2026 Shane Smith / Sassy Consulting LLC. All rights reserved.
// Proprietary source. This notice is Copyright Management Information (17 U.S.C. 1202); removal or alteration prohibited.
// CodeMark: SCLLC1-sassytalkie-2TQ53KBUNC6R
// App.tsx - Sassy-Talk Main UI with Lobby System
// Copyright 2025 Sassy Consulting LLC. All rights reserved.
// Production Build - Full Implementation

import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import './styles/app.css';
import './styles/lobby.css';
import PeerList from './components/lobby/PeerList';
import DeviceList from './components/DeviceList';
import ChannelSelector from './components/ChannelSelector';
import StatusBar from './components/StatusBar';
import SettingsPanel from './components/SettingsPanel';
import Sounds from './sounds';
import {
  IconLobby,
  IconRadio,
  IconSettings,
  IconSearch,
  IconClose,
  IconMic,
  IconSpeaker,
  IconRecording,
  IconRefresh,
} from './components/Icons';
import type {
  PeerInfo,
  AppStatus,
  DeviceInfo,
  Volume,
  AudioDevices,
  NetworkInfo,
  CellularStatus,
  View,
} from './types';

// ============================================================================
// Main App Component
// ============================================================================

export default function App() {
  // View state
  const [currentView, setCurrentView] = useState<View>('lobby');

  // Lobby state
  const [peers, setPeers] = useState<PeerInfo[]>([]);
  const [isSearching, setIsSearching] = useState(false);

  // Walkie state
  const [status, setStatus] = useState<AppStatus | null>(null);
  const [isTransmitting, setIsTransmitting] = useState(false);
  const [isReceiving, setIsReceiving] = useState(false);
  const [channel, setChannel] = useState(1);
  const [deviceInfo, setDeviceInfo] = useState<DeviceInfo | null>(null);

  // Settings state
  const [micVolume, setMicVolume] = useState(80);
  const [speakerVolume, setSpeakerVolume] = useState(80);
  const [rogerBeep, setRogerBeep] = useState(true);
  const [voxEnabled, setVoxEnabled] = useState(false);
  const [voxThreshold, setVoxThreshold] = useState(10);
  const [audioDevices, setAudioDevices] = useState<AudioDevices>({ inputs: [], outputs: [] });
  const [selectedInput, setSelectedInput] = useState<string>('');
  const [selectedOutput, setSelectedOutput] = useState<string>('');

  // Network settings state
  const [networkInfo, setNetworkInfo] = useState<NetworkInfo | null>(null);
  const [encryptionEnabled, setEncryptionEnabled] = useState(true);
  const [randomPortEnabled, setRandomPortEnabled] = useState(true);

  // Cellular relay (internet transport) state
  const [cellularQr, setCellularQr] = useState('');
  const [cellularRoom, setCellularRoom] = useState<string | null>(null);
  const [cellularStatus, setCellularStatus] = useState<CellularStatus | null>(null);
  const [cellularJoining, setCellularJoining] = useState(false);

  // Audio visualization
  const [audioLevel, setAudioLevel] = useState(0);

  // Error handling
  const [error, setError] = useState<string | null>(null);

  // Settings modal
  const [showSettingsModal, setShowSettingsModal] = useState(false);

  // Refs
  const pttButtonRef = useRef<HTMLButtonElement>(null);
  const statusIntervalRef = useRef<number | null>(null);
  // Mirror isSearching into a ref so the once-mounted init/polling effect can read
  // the latest value WITHOUT re-running (which previously reset wasReceiving and
  // caused spurious incoming chimes when the poll effect re-registered).
  const isSearchingRef = useRef(isSearching);
  useEffect(() => { isSearchingRef.current = isSearching; }, [isSearching]);

  // ============================================================================
  // Initialization
  // ============================================================================

  useEffect(() => {
    const setup = async () => {
      try {
        // Get device info
        const info = await invoke<DeviceInfo>('get_device_info');
        setDeviceInfo(info);

        // Get initial status
        const initialStatus = await invoke<AppStatus>('get_status');
        setStatus(initialStatus);
        setChannel(initialStatus.channel);

        // Get volume
        const vol = await invoke<Volume>('get_volume');
        setMicVolume(Math.round(vol.input * 100));
        setSpeakerVolume(Math.round(vol.output * 100));

        // Get audio devices
        const devices = await invoke<AudioDevices>('get_audio_devices');
        setAudioDevices(devices);

        // Set defaults
        const defaultInput = devices.inputs.find(d => d.is_default);
        const defaultOutput = devices.outputs.find(d => d.is_default);
        if (defaultInput) setSelectedInput(defaultInput.name);
        if (defaultOutput) setSelectedOutput(defaultOutput.name);

        // Get network info
        try {
          const netInfo = await invoke<NetworkInfo>('get_network_info');
          setNetworkInfo(netInfo);
          setEncryptionEnabled(netInfo.encryption_enabled);
          setRandomPortEnabled(netInfo.use_random_port);
        } catch (e) {
          console.warn('Failed to get network info:', e);
        }

      } catch (e) {
        console.error('Failed to initialize:', e);
        setError(`Initialization failed: ${e}`);
      }
    };

    setup();

    // Status polling drives receiving / audio-level UI. Backend never emitted
    // `audio_level` / `receiving` events; do not subscribe to dead event names.
    let hadPeers = false;
    let wasReceiving = false;

    // Status polling (250ms for status + peers, no network info here)
    statusIntervalRef.current = window.setInterval(async () => {
      try {
        const s = await invoke<AppStatus>('get_status');
        setStatus(s);
        setIsTransmitting(s.is_transmitting);
        setIsReceiving(s.is_receiving);
        // No meter on the backend — approximate the bar from TX/RX state.
        setAudioLevel(s.is_transmitting || s.is_receiving ? 70 : 0);
        if (s.is_receiving && !wasReceiving) {
          Sounds.incomingTransmission();
        }
        wasReceiving = s.is_receiving;

        if (isSearchingRef.current) {
          const nearbyPeers = await invoke<PeerInfo[]>('get_nearby_devices');
          // Play connection success when first peer discovered
          if (nearbyPeers.length > 0 && !hadPeers) {
            Sounds.connectionSuccess();
            hadPeers = true;
          } else if (nearbyPeers.length === 0) {
            hadPeers = false;
          }
          setPeers(nearbyPeers);
        }

        // Cellular relay status — authoritative, so a backend-side leave
        // (e.g. via the disconnect command) is reflected in the UI too.
        const cs = await invoke<string>('get_cellular_status');
        if (cs) {
          const parsed = JSON.parse(cs) as CellularStatus;
          setCellularStatus(parsed);
          setCellularRoom(parsed.room || null);
        } else {
          setCellularStatus(null);
          setCellularRoom(null);
        }
      } catch (e) {
        // Ignore polling errors
      }
    }, 250);

    return () => {
      if (statusIntervalRef.current) clearInterval(statusIntervalRef.current);
    };
    // Run ONCE on mount: setup + polling interval live for the component's
    // lifetime. isSearching is read via isSearchingRef inside the interval.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ==========================================================================
  // Lobby Functions
  // ==========================================================================

  const enterLobby = async () => {
    setError(null);
    setIsSearching(true);
    Sounds.discoveryStart();
    try {
      await invoke('start_discovery');
    } catch (e) {
      console.error('Failed to start discovery:', e);
      setError(`Discovery failed: ${e}`);
      setIsSearching(false);
      Sounds.error();
    }
  };

  const leaveLobby = async () => {
    setIsSearching(false);
    Sounds.discoveryStop();
    try {
      await invoke('stop_discovery');
    } catch (e) {
      console.error('Failed to stop discovery:', e);
    }
    setPeers([]);
  };

  const joinPeerChannel = async (peerChannel: number) => {
    try {
      await invoke('set_channel', { channel: peerChannel });
      setChannel(peerChannel);
      Sounds.connectionSuccess(); // Three-tone success chime
      setCurrentView('walkie');
    } catch (e) {
      setError(`Failed to join channel: ${e}`);
      Sounds.error();
    }
  };

  // ============================================================================
  // Walkie Functions
  // ============================================================================

  const startTransmit = async () => {
    if (isTransmitting) return;
    setError(null);
    try {
      await invoke('start_transmit');
      setIsTransmitting(true);
      Sounds.pttStart();
    } catch (e) {
      console.error('Failed to start transmit:', e);
      setError(`Transmit failed: ${e}`);
      Sounds.error();
    }
  };

  const stopTransmit = async () => {
    if (!isTransmitting) return;
    try {
      await invoke('stop_transmit');
      setIsTransmitting(false);
      // Roger beep is handled by the backend (plays locally via CPAL + sends to peers)
    } catch (e) {
      console.error('Failed to stop transmit:', e);
    }
  };

  const handleChannelChange = async (newChannel: number) => {
    if (newChannel === channel) return;

    setChannel(newChannel);
    try {
      await invoke('set_channel', { channel: newChannel });
      Sounds.channelChange();
    } catch (e) {
      console.error('Failed to change channel:', e);
      setError(`Channel change failed: ${e}`);
      Sounds.error();
    }
  };

  const disconnect = async () => {
    try {
      await invoke('disconnect');
      setIsSearching(false);
      setPeers([]);
      setCurrentView('lobby');
    } catch (e) {
      console.error('Failed to disconnect:', e);
    }
  };

  // ============================================================================
  // Settings Functions
  // ============================================================================

  const updateVolume = async (input: number, output: number) => {
    try {
      await invoke('set_volume', { input: input / 100, output: output / 100 });
    } catch (e) {
      console.error('Failed to set volume:', e);
    }
  };

  const handleMicVolumeChange = (value: number) => {
    setMicVolume(value);
    updateVolume(value, speakerVolume);
  };

  const handleSpeakerVolumeChange = (value: number) => {
    setSpeakerVolume(value);
    updateVolume(micVolume, value);
  };

  const handleRogerBeepChange = async (enabled: boolean) => {
    setRogerBeep(enabled);
    try {
      await invoke('set_roger_beep', { enabled });
    } catch (e) {
      console.error('Failed to set roger beep:', e);
    }
  };

  const handleVoxChange = async (enabled: boolean) => {
    setVoxEnabled(enabled);
    try {
      await invoke('set_vox_enabled', { enabled });
    } catch (e) {
      console.error('Failed to set VOX:', e);
    }
  };

  const handleVoxThresholdChange = async (value: number) => {
    setVoxThreshold(value);
    try {
      await invoke('set_vox_threshold', { threshold: value / 100 });
    } catch (e) {
      console.error('Failed to set VOX threshold:', e);
    }
  };

  const handleInputDeviceChange = async (deviceName: string) => {
    setSelectedInput(deviceName);
    try {
      await invoke('set_input_device', { deviceName });
    } catch (e) {
      setError(`Failed to set microphone: ${e}`);
    }
  };

  const handleOutputDeviceChange = async (deviceName: string) => {
    setSelectedOutput(deviceName);
    try {
      await invoke('set_output_device', { deviceName });
    } catch (e) {
      setError(`Failed to set speaker: ${e}`);
    }
  };

  const refreshAudioDevices = async () => {
    try {
      const devices = await invoke<AudioDevices>('get_audio_devices');
      setAudioDevices(devices);
    } catch (e) {
      console.error('Failed to refresh audio devices:', e);
    }
  };

  // ==========================================================================
  // Network Settings Handlers
  // ==========================================================================

  const handleEncryptionChange = async (enabled: boolean) => {
    setEncryptionEnabled(enabled);
    try {
      await invoke('set_encryption_enabled', { enabled });
      // Refresh network info
      const netInfo = await invoke<NetworkInfo>('get_network_info');
      setNetworkInfo(netInfo);
    } catch (e) {
      console.error('Failed to set encryption:', e);
      setError(`Failed to set encryption: ${e}`);
    }
  };

  const handleRandomPortChange = async (enabled: boolean) => {
    setRandomPortEnabled(enabled);
    try {
      await invoke('set_random_port_enabled', { enabled });
      // Note: Port change takes effect on next session
    } catch (e) {
      console.error('Failed to set random port:', e);
      setError(`Failed to set random port: ${e}`);
    }
  };

  // ==========================================================================
  // Cellular Relay (Internet) Handlers
  // ==========================================================================

  const joinCellular = async () => {
    const input = cellularQr.trim();
    if (!input || cellularJoining) return;
    setError(null);
    setCellularJoining(true);
    try {
      // Accept either a raw QR JSON payload or a one-time invite LINK
      // (https://relay.sassyconsultingllc.com/v/<id>#<key>). A link is fetched
      // and decrypted natively (import_share_link → shared core); pasted JSON
      // joins directly. Tauri v2 camelCases params: Rust `qr_json` -> `qrJson`.
      const isLink = /^https:\/\/relay\.sassyconsultingllc\.com\/v\//i.test(input);
      const room = isLink
        ? await invoke<string>('import_share_link', { url: input })
        : await invoke<string>('join_cellular_session', { qrJson: input });
      setCellularRoom(room);
      Sounds.connectionSuccess();
      // Jump to the Talk view so the user can immediately push-to-talk.
      setCurrentView('walkie');
    } catch (e) {
      console.error('Failed to join cellular session:', e);
      setError(`Join failed: ${e}`);
      Sounds.error();
    } finally {
      setCellularJoining(false);
    }
  };

  const leaveCellular = async () => {
    try {
      await invoke('leave_cellular_session');
    } catch (e) {
      console.error('Failed to leave cellular session:', e);
    }
    setCellularRoom(null);
    setCellularStatus(null);
    Sounds.discoveryStop();
  };

  // ============================================================================
  // PTT Handlers
  // ============================================================================

  const handlePttDown = useCallback(() => {
    // Allow PTT when local discovery is active OR a cellular relay session is
    // joined (TX fans out to every live plane so mixed-protocol peers hear us).
    if (!isTransmitting && (isSearching || cellularRoom !== null)) {
      startTransmit();
    }
  }, [isTransmitting, isSearching, cellularRoom]);

  const handlePttUp = useCallback(() => {
    if (isTransmitting) {
      stopTransmit();
    }
  }, [isTransmitting]);

  // Keyboard PTT
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.code === 'Space' && !e.repeat && currentView === 'walkie') {
        e.preventDefault();
        handlePttDown();
      }
    };

    const handleKeyUp = (e: KeyboardEvent) => {
      if (e.code === 'Space' && currentView === 'walkie') {
        e.preventDefault();
        handlePttUp();
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    window.addEventListener('keyup', handleKeyUp);

    return () => {
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('keyup', handleKeyUp);
    };
  }, [currentView, handlePttDown, handlePttUp]);

  // True when the user can transmit: either local discovery is active OR a
  // cellular relay session is joined (internet transport).
  const canTalk = isSearching || cellularRoom !== null;

  // ============================================================================
  // Render: Lobby View
  // ============================================================================

  const renderLobby = () => (
    <div className="view lobby-view">
      <header className="lobby-header">
        <h1>Sassy-Talk</h1>
        <div className="lobby-header-actions">
          <p className="subtitle">Bluetooth Walkie-Talkie</p>
          <button className="settings-gear-btn" onClick={() => setShowSettingsModal(true)} title="Quick Settings">
            <IconSettings size={20} />
          </button>
        </div>
      </header>

      {error && (
        <div className="error-banner">
          <span>{error}</span>
          <button onClick={() => setError(null)}><IconClose size={16} /></button>
        </div>
      )}

      <div className="search-section">
        <button
          className={`search-btn ${isSearching ? 'active' : ''}`}
          onClick={isSearching ? leaveLobby : enterLobby}
        >
          <span className="search-icon">{isSearching ? <IconSearch size={20} /> : <IconLobby size={20} />}</span>
          <span>{isSearching ? 'Searching...' : 'Find Devices'}</span>
        </button>
        {isSearching && (
          <div className="search-pulse">
            <div className="pulse-ring"></div>
            <div className="pulse-ring delay-1"></div>
            <div className="pulse-ring delay-2"></div>
          </div>
        )}
      </div>

      <div className="current-channel-display">
        <span className="channel-label">Your Channel:</span>
        <span className="channel-value">CH{channel.toString().padStart(2, '0')}</span>
      </div>

      <div className="peers-section">
        <h3>Nearby Devices {peers.length > 0 && `(${peers.length})`}</h3>

        {peers.length === 0 && isSearching && (
          <div className="no-peers">
            <p>Looking for nearby devices...</p>
            <p className="hint">Make sure other devices have the app open</p>
          </div>
        )}

        {peers.length === 0 && !isSearching && (
          <div className="no-peers">
            <p>No devices found</p>
            <p className="hint">Tap "Find Devices" to start searching</p>
          </div>
        )}

        <PeerList peers={peers} channel={channel} onJoin={joinPeerChannel} onTalk={() => setCurrentView('walkie')} />

        {peers.length > 0 && (
          <DeviceList peers={peers} currentChannel={channel} />
        )}
      </div>

      {isSearching && (
        <div className="quick-talk-section">
          <button className="quick-talk-btn" onClick={() => setCurrentView('walkie')}>
            <IconRadio size={20} />
            <span>Start Talking on CH{channel.toString().padStart(2, '0')}</span>
          </button>
        </div>
      )}

      <nav className="bottom-nav">
        <button className="nav-btn active">
          <span className="nav-icon"><IconLobby size={20} /></span>
          <span>Lobby</span>
        </button>
        <button className="nav-btn" onClick={() => setCurrentView('walkie')}>
          <span className="nav-icon"><IconRadio size={20} /></span>
          <span>Talk</span>
        </button>
        <button className="nav-btn" onClick={() => setCurrentView('settings')}>
          <span className="nav-icon"><IconSettings size={20} /></span>
          <span>Settings</span>
        </button>
      </nav>
    </div>
  );

  // ============================================================================
  // Render: Walkie View
  // ============================================================================

  const renderWalkie = () => (
    <div className="view walkie-view">
      {error && (
        <div className="error-banner">
          <span>{error}</span>
          <button onClick={() => setError(null)}><IconClose size={16} /></button>
        </div>
      )}

      <header className="walkie-header">
        <div className="connection-info">
          <StatusBar status={status} isConnected={canTalk} peerCount={peers.length} />
        </div>
        <div className="walkie-header-actions">
          <button className="settings-gear-btn" onClick={() => setShowSettingsModal(true)} title="Quick Settings">
            <IconSettings size={20} />
          </button>
          <button className="disconnect-btn" onClick={disconnect} title="Disconnect"><IconClose size={20} /></button>
        </div>
      </header>

      <ChannelSelector channel={channel} onChange={handleChannelChange} />

      <div className="status-display">
        <div className={`status-indicator ${isTransmitting ? 'tx' : isReceiving ? 'rx-active' : 'rx'}`}>
          {isTransmitting ? 'TRANSMITTING' : isReceiving ? 'RECEIVING' : 'STANDBY'}
        </div>
        <div className="audio-meter">
          <div className={`audio-level ${isTransmitting ? 'tx' : isReceiving ? 'rx' : ''}`} style={{ width: `${audioLevel}%` }}></div>
        </div>
      </div>

      <div className="ptt-container">
        <button
          ref={pttButtonRef}
          className={`ptt-btn ${isTransmitting ? 'active' : ''} ${!canTalk ? 'disabled' : ''}`}
          onMouseDown={handlePttDown}
          onMouseUp={handlePttUp}
          onMouseLeave={handlePttUp}
          onTouchStart={(e) => { e.preventDefault(); handlePttDown(); }}
          onTouchEnd={(e) => { e.preventDefault(); handlePttUp(); }}
          disabled={!canTalk}
        >
          <span className="ptt-icon">{isTransmitting ? <IconRecording size={48} /> : <IconMic size={48} />}</span>
          <span className="ptt-text">
            {!canTalk ? 'CONNECT FIRST' : isTransmitting ? 'RELEASE TO STOP' : 'PUSH TO TALK'}
          </span>
          <span className="ptt-hint">or hold SPACEBAR</span>
        </button>
      </div>

      <div className="quick-settings">
        <div className="volume-control">
          <span className="volume-icon"><IconSpeaker size={20} /></span>
          <input type="range" min="0" max="100" value={speakerVolume} onChange={(e) => handleSpeakerVolumeChange(Number(e.target.value))} />
          <span className="volume-value">{speakerVolume}%</span>
        </div>
        <div className="volume-control">
          <span className="volume-icon"><IconMic size={20} /></span>
          <input type="range" min="0" max="100" value={micVolume} onChange={(e) => handleMicVolumeChange(Number(e.target.value))} />
          <span className="volume-value">{micVolume}%</span>
        </div>
      </div>

      {peers.length > 0 && (
        <div className="nearby-mini">
          <span className="nearby-label">Nearby:</span>
          {peers.slice(0, 3).map(p => (
            <span key={p.device_id} className="nearby-peer">{p.device_name.substring(0, 10)}</span>
          ))}
          {peers.length > 3 && <span className="nearby-more">+{peers.length - 3} more</span>}
        </div>
      )}

      <nav className="bottom-nav">
        <button className="nav-btn" onClick={() => setCurrentView('lobby')}>
          <span className="nav-icon"><IconLobby size={20} /></span>
          <span>Lobby</span>
        </button>
        <button className="nav-btn active">
          <span className="nav-icon"><IconRadio size={20} /></span>
          <span>Talk</span>
        </button>
        <button className="nav-btn" onClick={() => setCurrentView('settings')}>
          <span className="nav-icon"><IconSettings size={20} /></span>
          <span>Settings</span>
        </button>
      </nav>
    </div>
  );

  // ============================================================================
  // Render: Settings View
  // ============================================================================

  const renderSettings = () => (
    <div className="view settings-view">
      <header className="settings-header">
        <h1>Settings</h1>
      </header>

      {error && (
        <div className="error-banner">
          <span>{error}</span>
          <button onClick={() => setError(null)}><IconClose size={16} /></button>
        </div>
      )}

      <div className="settings-content">
        <section className="settings-section">
          <h3>Device</h3>
          <div className="setting-row">
            <span className="setting-label">Device ID</span>
            <span className="setting-value">{deviceInfo?.device_id || '--------'}</span>
          </div>
          <div className="setting-row">
            <span className="setting-label">Device Name</span>
            <span className="setting-value">{deviceInfo?.device_name || 'Unknown'}</span>
          </div>
          <div className="setting-row">
            <span className="setting-label">Version</span>
            <span className="setting-value">{deviceInfo?.version || '0.0.0'}</span>
          </div>
        </section>

        <section className="settings-section">
          <h3>Audio Devices <button className="refresh-btn" onClick={refreshAudioDevices} title="Refresh"><IconRefresh size={16} /></button></h3>
          <div className="setting-row">
            <span className="setting-label">Microphone</span>
            <select value={selectedInput} onChange={(e) => handleInputDeviceChange(e.target.value)} className="device-select">
              {audioDevices.inputs.map(d => (
                <option key={d.name} value={d.name}>{d.name} {d.is_default ? '(Default)' : ''}</option>
              ))}
            </select>
          </div>
          <div className="setting-row">
            <span className="setting-label">Speaker</span>
            <select value={selectedOutput} onChange={(e) => handleOutputDeviceChange(e.target.value)} className="device-select">
              {audioDevices.outputs.map(d => (
                <option key={d.name} value={d.name}>{d.name} {d.is_default ? '(Default)' : ''}</option>
              ))}
            </select>
          </div>
        </section>

        <section className="settings-section">
          <h3>Audio Levels</h3>
          <div className="setting-row">
            <span className="setting-label">Mic Volume</span>
            <input type="range" min="0" max="100" value={micVolume} onChange={(e) => handleMicVolumeChange(Number(e.target.value))} />
            <span className="setting-value">{micVolume}%</span>
          </div>
          <div className="setting-row">
            <span className="setting-label">Speaker Volume</span>
            <input type="range" min="0" max="100" value={speakerVolume} onChange={(e) => handleSpeakerVolumeChange(Number(e.target.value))} />
            <span className="setting-value">{speakerVolume}%</span>
          </div>
        </section>

        <section className="settings-section">
          <h3>Voice Features</h3>
          <div className="setting-row">
            <span className="setting-label">Roger Beep</span>
            <label className="toggle">
              <input type="checkbox" checked={rogerBeep} onChange={(e) => handleRogerBeepChange(e.target.checked)} />
              <span className="toggle-slider"></span>
            </label>
          </div>
          <div className="setting-row">
            <span className="setting-label">VOX (Voice Activated)</span>
            <label className="toggle">
              <input type="checkbox" checked={voxEnabled} onChange={(e) => handleVoxChange(e.target.checked)} />
              <span className="toggle-slider"></span>
            </label>
          </div>
          {voxEnabled && (
            <div className="setting-row">
              <span className="setting-label">VOX Threshold</span>
              <input type="range" min="0" max="100" value={voxThreshold} onChange={(e) => handleVoxThresholdChange(Number(e.target.value))} />
              <span className="setting-value">{voxThreshold}%</span>
            </div>
          )}
        </section>

        <section className="settings-section">
          <h3>Network</h3>
          <div className="setting-row">
            <span className="setting-label">Transport</span>
            <span className="setting-value">UDP Multicast</span>
          </div>
          <div className="setting-row">
            <span className="setting-label">Multicast Group</span>
            <span className="setting-value">{networkInfo?.multicast_addr || '239.255.42.42'}</span>
          </div>
          <div className="setting-row">
            <span className="setting-label">Port</span>
            <span className="setting-value">{networkInfo?.port || '---'} {networkInfo?.use_random_port ? '(random)' : '(fixed)'}</span>
          </div>
          <div className="setting-row">
            <span className="setting-label">Random Port Each Session</span>
            <label className="toggle">
              <input type="checkbox" checked={randomPortEnabled} onChange={(e) => handleRandomPortChange(e.target.checked)} />
              <span className="toggle-slider"></span>
            </label>
          </div>
        </section>

        <section className="settings-section">
          <h3>Internet Relay</h3>
          {cellularRoom === null ? (
            <>
              <p className="cellular-hint">
                Join a session over the internet — no Wi-Fi or Bluetooth needed. Paste an
                invite <strong>link</strong> (https://relay.sassyconsultingllc.com/v/…) or the
                session QR data from the phone app's "Show QR" screen.
              </p>
              <textarea
                className="cellular-qr-input"
                placeholder={'https://relay.sassyconsultingllc.com/v/<id>#<key>\n— or —\n{"key":"...","session_id":"...","channel":1}'}
                value={cellularQr}
                onChange={(e) => setCellularQr(e.target.value)}
                rows={4}
                spellCheck={false}
              />
              <button
                className="cellular-join-btn"
                onClick={joinCellular}
                disabled={cellularJoining || cellularQr.trim().length === 0}
              >
                {cellularJoining ? 'Connecting…' : 'Join over Internet'}
              </button>
            </>
          ) : (
            <>
              <div className="setting-row">
                <span className="setting-label">Status</span>
                <span className={`setting-value ${cellularStatus?.state === 'connected' ? 'secure' : 'insecure'}`}>
                  {cellularStatus?.state ?? 'connecting'}
                </span>
              </div>
              <div className="setting-row">
                <span className="setting-label">Room</span>
                <span className="setting-value key-value" title={cellularRoom}>
                  {cellularRoom.slice(0, 18)}{cellularRoom.length > 18 ? '…' : ''}
                </span>
              </div>
              <div className="setting-row">
                <span className="setting-label">Frames (tx / rx)</span>
                <span className="setting-value">
                  {cellularStatus?.sent ?? 0} / {cellularStatus?.received ?? 0}
                </span>
              </div>
              <button className="cellular-leave-btn" onClick={leaveCellular}>
                Leave Internet Session
              </button>
            </>
          )}
        </section>

        <section className="settings-section">
          <h3>Security</h3>
          <div className="setting-row">
            <span className="setting-label">End-to-End Encryption</span>
            <label className="toggle">
              <input type="checkbox" checked={encryptionEnabled} onChange={(e) => handleEncryptionChange(e.target.checked)} />
              <span className="toggle-slider"></span>
            </label>
          </div>
          <div className="setting-row">
            <span className="setting-label">Encryption Status</span>
            <span className={`setting-value ${networkInfo?.is_encrypted ? 'secure' : 'insecure'}`}>
              {networkInfo?.is_encrypted ? 'Active' : 'Inactive'}
            </span>
          </div>
          {networkInfo?.public_key && (
            <div className="setting-row">
              <span className="setting-label">Public Key</span>
              <span className="setting-value key-value" title={networkInfo.public_key}>
                {networkInfo.public_key.substring(0, 16)}...
              </span>
            </div>
          )}
          <div className="setting-row">
            <span className="setting-label">Key Exchange</span>
            <span className="setting-value">X25519 ECDH</span>
          </div>
          <div className="setting-row">
            <span className="setting-label">Cipher</span>
            <span className="setting-value">AES-256-GCM</span>
          </div>
        </section>

        <section className="settings-section">
          <h3>Audio Codec</h3>
          <div className="setting-row">
            <span className="setting-label">Codec</span>
            <span className="setting-value">Opus 32kbps VBR</span>
          </div>
          <div className="setting-row">
            <span className="setting-label">Frame Size</span>
            <span className="setting-value">20ms (960 samples)</span>
          </div>
        </section>

        <section className="settings-section">
          <h3>About</h3>
          <div className="setting-row">
            <span className="setting-label">&copy; 2025 Sassy Consulting LLC</span>
          </div>
        </section>
      </div>

      <nav className="bottom-nav">
        <button className="nav-btn" onClick={() => setCurrentView('lobby')}>
          <span className="nav-icon"><IconLobby size={20} /></span>
          <span>Lobby</span>
        </button>
        <button className="nav-btn" onClick={() => setCurrentView('walkie')}>
          <span className="nav-icon"><IconRadio size={20} /></span>
          <span>Talk</span>
        </button>
        <button className="nav-btn active">
          <span className="nav-icon"><IconSettings size={20} /></span>
          <span>Settings</span>
        </button>
      </nav>
    </div>
  );

  // ============================================================================
  // Main Render
  // ============================================================================

  return (
    <div className="app">
      {currentView === 'lobby' && renderLobby()}
      {currentView === 'walkie' && renderWalkie()}
      {currentView === 'settings' && renderSettings()}

      {showSettingsModal && (
        <SettingsPanel onClose={() => setShowSettingsModal(false)} />
      )}
    </div>
  );
}
