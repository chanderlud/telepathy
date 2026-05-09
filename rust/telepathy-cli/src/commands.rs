use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ChatAttachment {
    pub name: String,
    pub data_b64: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "cmd", content = "args", rename_all = "snake_case")]
pub enum Command {
    SetIdentity {
        key_b64: String,
    },
    AddContact {
        id: String,
        nickname: String,
        peer_id: String,
    },
    RemoveContact {
        id: String,
    },
    StartManager,
    RestartManager,
    Shutdown,
    StartSession {
        contact_id: String,
    },
    StopSession {
        contact_id: String,
    },
    StartCall {
        contact_id: String,
    },
    EndCall,
    AcceptCall {
        request_id: String,
        accept: bool,
    },
    JoinRoom {
        members: Vec<String>,
    },
    SendChat {
        contact_id: String,
        text: String,
        attachments: Vec<ChatAttachment>,
    },
    AudioTest,
    SetMuted {
        value: bool,
    },
    SetDeafened {
        value: bool,
    },
    SetInputVolumeDb {
        value: f32,
    },
    SetOutputVolumeDb {
        value: f32,
    },
    SetRmsThresholdDb {
        value: f32,
    },
    SetDenoise {
        value: bool,
    },
    SetEfficiencyMode {
        value: bool,
    },
    SetPlayCustomRingtones {
        value: bool,
    },
    SetInputDevice {
        id: Option<String>,
    },
    SetOutputDevice {
        id: Option<String>,
    },
    ListDevices,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Envelope {
    pub id: String,
    #[serde(flatten)]
    pub cmd: Command,
}
