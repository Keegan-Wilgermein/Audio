// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// -------- Imports --------
use std::{error::Error as STDError, ffi::OsString, fs::{self, remove_file, rename}, sync::{Arc, Mutex, RwLock}, thread::{self, Thread}, time::{Duration, Instant}};
use savefile::{load_file, save_file};
use savefile_derive::Savefile;
use slint::{Model, ModelRc, SharedString, ToSharedString, VecModel};
use qruhear::{RUHear, RUBuffers, rucallback};
use hound::{WavWriter, SampleFormat, WavSpec};
use kira::{effect::{eq_filter::{EqFilterBuilder, EqFilterKind}, panning_control::PanningControlBuilder}, sound::static_sound::StaticSoundData, track::{TrackBuilder}, AudioManager, AudioManagerSettings, DefaultBackend, Tween};

slint::include_modules!();

// -------- Enums --------
// Errors
#[derive(Clone, Copy, PartialEq)]
enum Error {
    SaveError,
    LoadError,
    RecordError,
    WriteError,
    ReadError,
    RenameError,
    DeleteError,
    FallbackError,
    EmptyError,
    ExistsError,
    SaveFileRenameError,
    PlaybackError,
    ControllerError,
}

impl Error {
    fn get_text(self) -> SharedString {
        match self {
            Error::SaveError => SharedString::from("Failed to save data"),
            Error::LoadError => SharedString::from("Data doesn't exist"),
            Error::RecordError => SharedString::from("Recording failed"),
            Error::WriteError => SharedString::from("Failed to write audio"),
            Error::ReadError => SharedString::from("File read failed"),
            Error::RenameError => SharedString::from("Failed to rename file"),
            Error::DeleteError => SharedString::from("Failed to delete file"),
            Error::FallbackError => SharedString::from("Can't rename to fallback name"),
            Error::EmptyError => SharedString::from("Name has to contain something"),
            Error::ExistsError => SharedString::from("Name already exists"),
            Error::SaveFileRenameError => SharedString::from("Can't rename to 'settings'"),
            Error::PlaybackError => SharedString::from("Failed to play audio"),
            Error::ControllerError => SharedString::from("Audio controller crashed"),
        }
    }
}

// File stuff
#[derive(PartialEq)]
enum File {
    Names(Vec<String>),
}

impl File {
    fn search(path: &str, extension: &str) -> Result<File, Error> {
        let mut names = vec![];
        match fs::read_dir(path) {
            Ok(directories) => {
                for entry in directories {
                    match entry {
                        Ok(directory) => {
                            let path = directory.path();

                            if path.is_file() {
                                if let Some(file_type) = path.extension() {
                                    if file_type == extension {
                                        let file_name = match path.file_name() {
                                            Some(value) => value.to_owned(),
                                            None => OsString::from("Couldn't read name"),
                                        };
                                        names.push(match file_name.into_string() {
                                            Ok(mut value) => {
                                                File::truncate(&mut value)
                                            },
                                            Err(_) => String::from("Couldn't read name"),
                                        });
                                    }
                                }
                            }
                        },
                        Err(_) => {
                            return Err(Error::ReadError);
                        },
                    }
                }
                names.sort();
                Ok(File::Names(names))
            },
            Err(_) => Err(Error::ReadError),
        }
    }

    fn truncate(name: &mut String) -> String {
        let mut length = name.len() - 1;
        loop {
            if name.ends_with(".") {
                name.remove(length);
                break;
            } else {
                if length == 1 {
                    *name = String::from("Invalid file extension");
                }
                name.remove(length);
                length -= 1;
            }
        }

        name.to_string()
    }

    fn rename(old: &String, name: String) -> Option<Error> {
        match rename(format!("{}.wav", old), format!("{}.wav", name)) {
            Ok(_) => (),
            Err(_) => {
                return Some(Error::RenameError);
            },
        };

        match rename(format!("{}.bin", old), format!("{}.bin", name)) {
            Ok(_) => (),
            Err(_) => {
                return Some(Error::RenameError);
            },
        };

        None
    }

    fn delete(name: String) -> Option<Error> {
        match remove_file(format!("./{}.wav", name)) {
            Ok(_) => (),
            Err(_) => {
                return Some(Error::DeleteError);
            },
        };
        match remove_file(format!("./{}.bin", name)) {
            Ok(_) => None,
            Err(_) => Some(Error::DeleteError),
        }
    }

    fn exists(new: String, old_list: &Vec<Recording>) -> bool {
        let mut check = false;
        for item in 0..old_list.len() {
            if new == old_list[item].name {
                check = true;
                break;
            }
        }

        check
    }

    fn play(mut file: String, values: Arc<RwLock<Settings>>, selected_recording: usize, paused: Arc<RwLock<bool>>, snapping: bool, snap_playing: bool, mut snapshot: SnapShot, frame_updates: Arc<RwLock<[i32; 6]>>) -> Option<Error> {

        let state = match thread::Builder::new().name(String::from("Player")).spawn(move || {

            let mut audio_manager = match AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()) {
                Ok(value) => value,
                Err(_) => {
                    return Some(Error::ControllerError);
                }
            };

            // Filter setup
            let sub_bass = EqFilterBuilder::new(EqFilterKind::LowShelf, 40.0, 0.0, 1.0);
            let bass = EqFilterBuilder::new(EqFilterKind::Bell, 155.0, 0.0, 0.82);
            let low_mids = EqFilterBuilder::new(EqFilterKind::Bell, 625.0, 0.0, 0.83);
            let high_mids = EqFilterBuilder::new(EqFilterKind::Bell, 1500.0, 0.0, 1.5);
            let treble = EqFilterBuilder::new(EqFilterKind::HighShelf, 12000.0, 0.0, 0.75 );
            let pan = PanningControlBuilder::default();

            // Filter handles for real time updating
            let mut builder = TrackBuilder::new();
            let mut sub_bass_handle = builder.add_effect(sub_bass);
            let mut bass_handle = builder.add_effect(bass);
            let mut low_mids_handle = builder.add_effect(low_mids);
            let mut high_mids_handle = builder.add_effect(high_mids);
            let mut treble_handle = builder.add_effect(treble);
            let mut panning_handle = builder.add_effect(pan);

            let mut track = match audio_manager.add_sub_track(builder) {
                Ok(value) => value,
                Err(_) => {
                    let mut should_play = paused.write().unwrap();
                    *should_play = false;
                    return Some(Error::PlaybackError);
                }
            };

            let sound_data = match StaticSoundData::from_file(&file) {
                Ok(value) => value,
                Err(_) => {
                    let mut should_play = paused.write().unwrap();
                    *should_play = false;
                    return Some(Error::ReadError);
                }
            };

            let length = sound_data.duration();

            let _ = match track.play(sound_data) {
                Ok(value) => value,
                Err(_) => {
                    let mut should_play = paused.write().unwrap();
                    *should_play = false;
                    return Some(Error::PlaybackError);
                }
            };

            let start = Instant::now();
            let mut frame: usize = 0;
            let mut previous_frame = [0, 0, 0, 0, 0, 0];
            let mut edited_frame: usize = 0;
            while start.elapsed() < length {
                let should_play = paused.read().unwrap();
                if *should_play {
                    if snap_playing && !snapping {
                        if edited_frame < snapshot.frames.len() {
                            if frame == snapshot.frames[edited_frame].1 as usize {
                                let mut dial_values = frame_updates.write().unwrap();
                                *dial_values = snapshot.frames[edited_frame].0;
                                sub_bass_handle.set_gain(if snapshot.frames[edited_frame].0[0] == -7 {
                                    -60.0
                                } else {
                                    snapshot.frames[edited_frame].0[0] as f32 * 4.0
                                }, Tween::default());
                                bass_handle.set_gain(if snapshot.frames[edited_frame].0[1] == -7 {
                                    -60.0
                                } else {
                                    snapshot.frames[edited_frame].0[1] as f32 * 4.0
                                }, Tween::default());
                                low_mids_handle.set_gain(if snapshot.frames[edited_frame].0[2] == -7 {
                                    -60.0
                                } else {
                                    snapshot.frames[edited_frame].0[2] as f32 * 4.0
                                }, Tween::default());
                                high_mids_handle.set_gain(if snapshot.frames[edited_frame].0[3] == -7 {
                                    -60.0
                                } else {
                                    snapshot.frames[edited_frame].0[3] as f32 * 4.0
                                }, Tween::default());
                                treble_handle.set_gain(if snapshot.frames[edited_frame].0[4] == -7 {
                                    -60.0
                                } else {
                                    snapshot.frames[edited_frame].0[4] as f32 * 4.0
                                }, Tween::default());
                                panning_handle.set_panning(snapshot.frames[edited_frame].0[5] as f32 * 0.15, Tween::default());
    
                                edited_frame += 1;
                            }
                        }

                        frame += 1;

                    } else {
                        let value = values.read().unwrap();

                        if snapping {
                            if SnapShot::edited(previous_frame, Recording::parse(&value.recordings[selected_recording])) {
                                snapshot.frames.push((Recording::parse(&value.recordings[selected_recording]), frame as i32));
                                previous_frame = snapshot.frames[edited_frame].0;
                                edited_frame += 1;
                            }
                        }
                        
                        sub_bass_handle.set_gain(if value.recordings[selected_recording].sub_bass == -7 {
                            -60.0
                        } else {
                            value.recordings[selected_recording].sub_bass as f32 * 4.0
                        }, Tween::default());
                        bass_handle.set_gain(if value.recordings[selected_recording].bass == -7 {
                            -60.0
                        } else {
                            value.recordings[selected_recording].bass as f32 * 4.0
                        }, Tween::default());
                        low_mids_handle.set_gain(if value.recordings[selected_recording].low_mids == -7 {
                            -60.0
                        } else {
                            value.recordings[selected_recording].low_mids as f32 * 4.0
                        }, Tween::default());
                        high_mids_handle.set_gain(if value.recordings[selected_recording].high_mids == -7 {
                            -60.0
                        } else {
                            value.recordings[selected_recording].high_mids as f32 * 4.0
                        }, Tween::default());
                        treble_handle.set_gain(if value.recordings[selected_recording].treble == -7 {
                            -60.0
                        } else {
                            value.recordings[selected_recording].treble as f32 * 4.0
                        }, Tween::default());
                        panning_handle.set_panning(value.recordings[selected_recording].pan as f32 * 0.15, Tween::default());
                        
                        frame += 1;
                        drop(value);
                    }
                    
                    drop(should_play);
                    thread::sleep(Duration::from_millis(20));
                } else {
                    break;
                }
            }

            let mut should_play = paused.write().unwrap();
            *should_play = false;
            
            if snapping {
                snapshot.frames.remove(0);
                snapshot.save(&File::truncate(&mut file));
            }

            None
        }) {
            Ok(_) => None,
            Err(_) => Some(Error::PlaybackError),
        };

        state
    }

    fn stop(playing: Arc<RwLock<bool>>) {
        let mut should_play = playing.write().unwrap();
        *should_play = false;
        drop(should_play);
    }
}

enum DataType {
    Settings(Settings),
    SnapShot(SnapShot),
}

enum LoadType {
    Settings,
    Snapshot,
}

// -------- Structs --------
// Index data for Settings struct
struct IndexData {
    preset_length: usize,
    recording_length: usize,
}

// Snapshot struct
#[derive(Savefile, Clone)]
struct SnapShot {
    frames: Vec<([i32; 6], i32)>,
}

impl SnapShot {
    fn create(name: &str) -> Option<Error> {
        
        match SnapShot::save(SnapShot { frames: vec![([0, 0, 0, 0, 0, 0], 0)] }, name) {
            Some(error) => {
                return Some(error);
            },
            None => {
            }
        };
        
        None
    }

    fn new() -> SnapShot {
        SnapShot {
            frames: vec![([0, 0, 0, 0, 0, 0], 0)],
        }
    }

    fn edited(previous: [i32; 6], next: [i32; 6]) -> bool {
        for number in 0..6 {
            if previous[number] == next[number] {
                continue;
            } else {
                return true;
            }
        }

        false
    }

    fn save(self, name: &str) -> Option<Error> {
        save(DataType::SnapShot(self), name)
    }
}

// Preset data
#[derive(Savefile, Clone)]
struct Preset {
    name: String,
    sub_bass: i32,
    bass: i32,
    low_mids: i32,
    high_mids: i32,
    treble: i32,
    pan: i32,
}

impl Preset {
    fn from(values: [i32; 6]) -> Preset {
        Preset {
            name: String::from("New Preset"),
            sub_bass: values[0],
            bass: values[1],
            low_mids: values[2],
            high_mids: values[3],
            treble: values[4],
            pan: values[5],
        }
    }

    fn send_names(list: &Vec<Preset>, length: &usize) -> ModelRc<SharedString> {
        let mut preset_names = vec![];
        for preset in 0..*length {
            preset_names.push(list[preset].name.to_shared_string());
        }
        ModelRc::new(VecModel::from(preset_names))
    }

    fn send_values(list: &Vec<Preset>, length: &usize) -> ModelRc<ModelRc<i32>> {
        let mut all_preset_values = vec![];
        for values in 0..*length {
            let mut preset_values = vec![];
            
            preset_values.push(list[values].sub_bass);
            preset_values.push(list[values].bass);
            preset_values.push(list[values].low_mids);
            preset_values.push(list[values].high_mids);
            preset_values.push(list[values].treble);
            preset_values.push(list[values].pan);

            all_preset_values.push(ModelRc::new(VecModel::from(preset_values)));
        }
        ModelRc::new(VecModel::from(all_preset_values))
    }
}

// Recording data
#[derive(Savefile, Clone)]
struct Recording {
    name: String,
    sub_bass: i32,
    bass: i32,
    low_mids: i32,
    high_mids: i32,
    treble: i32,
    pan: i32,
}

impl Recording {
    fn new(name: &String) -> Recording {
        Recording {
            name: name.to_string(),
            sub_bass: 0,
            bass: 0,
            low_mids: 0,
            high_mids: 0,
            treble: 0,
            pan: 0,
        }
    }

    fn from(name: &String, values: [i32; 6]) -> Recording {
        Recording {
            name: name.to_string(),
            sub_bass: values[0],
            bass: values[1],
            low_mids: values[2],
            high_mids: values[3],
            treble: values[4],
            pan: values[5],
        }
    }

    fn parse(&self) -> [i32; 6] {
        let mut list: [i32; 6] = [0, 0, 0, 0, 0, 0];

        list[0] = self.sub_bass;
        list[1] = self.bass;
        list[2] = self.low_mids;
        list[3] = self.high_mids;
        list[4] = self.treble;
        list[5] = self.pan;

        list
    }

    fn parse_vec_from_recording(&self) -> Vec<i32> {
        let mut list = vec![];

        list.push(self.sub_bass);
        list.push(self.bass);
        list.push(self.low_mids);
        list.push(self.high_mids);
        list.push(self.treble);
        list.push(self.pan);

        list
    }

    fn parse_vec_from_list(list: [i32; 6]) -> Vec<i32> {
        let mut new = vec![];

        new.push(list[0]);
        new.push(list[1]);
        new.push(list[2]);
        new.push(list[3]);
        new.push(list[4]);
        new.push(list[5]);

        new
    }

    fn send_names(list: &Vec<Recording>) -> ModelRc<SharedString> {
        let mut new_list = vec![];

        for recording in 0..list.len() {
            new_list.push(list[recording].name.to_shared_string());
        }

        ModelRc::new(VecModel::from(new_list))
    }

    fn send_values(list: &Vec<Recording>, length: &usize) -> ModelRc<ModelRc<i32>> {
        let mut all_recording_values = vec![];
        for values in 0..*length {
            let mut recording_values = vec![];
            
            recording_values.push(list[values].sub_bass);
            recording_values.push(list[values].bass);
            recording_values.push(list[values].low_mids);
            recording_values.push(list[values].high_mids);
            recording_values.push(list[values].treble);
            recording_values.push(list[values].pan);

            all_recording_values.push(ModelRc::new(VecModel::from(recording_values)));
        }
        ModelRc::new(VecModel::from(all_recording_values))
    }

    fn rename(old: &Vec<Recording>, new: ModelRc<SharedString>) -> Result<Vec<Recording>, (Vec<Recording>, Error)> {
        let mut recording_list = vec![];

        let mut fallback_error_occured = false;
        let mut empty_error_occured = false;
        let mut exists_error_occured = false;
        let mut save_file_rename_error_occured = false;
        let mut rename_failed = (false, None);

        for name in 0..old.len() {
            if new.row_data(name).unwrap() != old[name].name {
                if new.row_data(name).unwrap().contains(&String::from("Default taken...")) {
                    recording_list.push(Recording::from(&old[name].name, old[name].parse()));
                    fallback_error_occured = true;
                } else if new.row_data(name).unwrap() == String::from("settings") {
                    recording_list.push(Recording::from(&old[name].name, old[name].parse()));
                    save_file_rename_error_occured = true;
                } else if new.row_data(name).unwrap().is_empty() || new.row_data(name).unwrap() == String::from("") {
                    recording_list.push(Recording::from(&old[name].name, old[name].parse()));
                    empty_error_occured = true;
                } else if File::exists(String::from(new.row_data(name).unwrap()), &old) {
                    recording_list.push(Recording::from(&old[name].name, old[name].parse()));
                    exists_error_occured = true;
                } else {
                    match File::rename(&old[name].name, String::from(new.row_data(name).unwrap())) {
                        Some(error) => {
                            rename_failed = (true, Some(error));
                        },
                        None => {
                        }
                    }
                    recording_list.push(Recording::from(&String::from(new.row_data(name).unwrap()), old[name].parse()));
                }
            } else {
                recording_list.push(Recording::from(&old[name].name, old[name].parse()));
            }
        }
        
        if exists_error_occured {
            Err((recording_list, Error::ExistsError))
        } else if empty_error_occured {
            Err((recording_list, Error::EmptyError))
        } else if fallback_error_occured {
            Err((recording_list, Error::FallbackError))
        } else if save_file_rename_error_occured {
            Err((recording_list, Error::SaveFileRenameError))
        } else if rename_failed.0 {
            Err((recording_list, rename_failed.1.unwrap()))
        } else {
            Ok(recording_list)
        }
    }

}

// All settings data
#[derive(Savefile, Clone)]
struct Settings {
    presets: Vec<Preset>,
    recordings: Vec<Recording>,
}

impl Settings {
    fn new() -> Settings {
        Settings {
            presets: vec![],
            recordings: vec![],
        }
    }

    fn get_index_data(&self) -> IndexData {
        IndexData {
            preset_length: self.presets.len(),
            recording_length: self.recordings.len(),
        }
    }

    fn sync(&mut self, ui: &AppWindow) {
        let index_data = self.get_index_data();

        let mut dials = [0, 0, 0, 0, 0, 0];
        for index in 0..6 {
            dials[index] = ui.get_dial_values().row_data(index).unwrap();
        }

        // Check for new preset creation
        if ui.get_new_preset() {
            self.presets.push(Preset::from(dials));
        }

        // Check for preset deletion
        if ui.get_delete_preset() {
            if self.presets.len() > ui.get_deleted_preset() as usize {
                self.presets.remove(ui.get_deleted_preset() as usize);
                ui.set_can_delete(true);
            }
        }

        // Check for preset rename
        if ui.get_rename_preset() {
            for preset in 0..index_data.preset_length {
                self.presets[preset].name = String::from(match ui.get_preset_names().row_data(preset) {
                    Some(name) => name,
                    None => SharedString::from("New Preset"),
                });
            }
        }

        // Check for recording edits
        if index_data.recording_length > 0 {
            let position = ui.get_current_recording() as usize;
            if ui.get_dials_edited() {
                self.recordings[position] = Recording::from(&self.recordings[position].name, dials);
            }
        }

        // Check for recording deletion
        if ui.get_delete_recording() {
            self.recordings.remove(ui.get_deleted_recording() as usize);
            ui.set_can_delete(true);
        }

        // Check for recording renaming
        if ui.get_renamed_recording() {
            self.recordings = match Recording::rename(&self.recordings, ui.get_recording_names()) {
                Ok(value) => value,
                Err(error) => {
                    ui.set_error_notification(error.1.get_text());
                    ui.set_error_recieved(true);
                    error.0
                },
            };
        }

        // Sync recording data with any changes that might have been made while the app was closed
        if ui.get_started() || ui.get_new_recording() {
            let file_names = match File::search("./", "wav") {
                Ok(File::Names(value)) => value,
                Err(error) => {
                    ui.set_error_notification(error.get_text());
                    ui.set_error_recieved(true);
                    vec![String::from("Couldn't read files")]
                }
            };

            let mut snapshot_names = match File::search("./", "bin") {
                Ok(File::Names(value)) => value,
                Err(error) => {
                    ui.set_error_notification(error.get_text());
                    ui.set_error_recieved(true);
                    vec![String::from("Couldn't read files")]
                }
            };

            for name in 0..snapshot_names.len() {
                if snapshot_names[name] == "settings" {
                    snapshot_names.remove(name);
                    break;
                }
            }

            let mut updated_recordings = vec![];

            for name in 0..file_names.len() {
                if self.recordings.len() > 0 {
                    for recording in 0..self.recordings.len() {
                        if self.recordings[recording].name == file_names[name] {
                            updated_recordings.push(Recording::from(&file_names[name], Recording::parse(&self.recordings[recording])));
                            break;
                        }
                        if recording == self.recordings.len() - 1 {
                            updated_recordings.push(Recording::new(&file_names[name]));
                        }
                    }
                } else {
                    updated_recordings.push(Recording::new(&file_names[name]));
                }

                // Syncs snapshots
                if snapshot_names.len() > 0 {
                    for file in 0..snapshot_names.len() {
                        if file_names[name] != snapshot_names[file] {
                            match SnapShot::create(&file_names[name]) {
                                Some(error) => {
                                    ui.set_error_notification(error.get_text());
                                    ui.set_error_recieved(true);
                                },
                                None => (),
                            }
                        } else {
                            snapshot_names.remove(file);
                            break;
                        }
                    }
                } else {
                    match SnapShot::create(&file_names[name]) {
                        Some(error) => {
                            ui.set_error_notification(error.get_text());
                            ui.set_error_recieved(true);
                        },
                        None => (),
                    }
                }
            }
            
            self.recordings = updated_recordings;
        }
    }
}

// Keeps track of the settings, the recording thread, whether recordings are being played, and the values of the dials during a set of audio frames
struct Tracker {
    settings: Arc<RwLock<Settings>>,
    recorder: Arc<Mutex<Option<Thread>>>,
    playing: Arc<RwLock<bool>>,
    snapshot_frame_values: Arc<RwLock<[i32; 6]>>
}

impl Tracker {
    fn new(settings: Settings) -> Tracker {
        Tracker {
            settings: Arc::new(RwLock::new(settings)),
            recorder: Arc::new(Mutex::new(None)),
            playing: Arc::new(RwLock::new(false)),
            snapshot_frame_values: Arc::new(RwLock::new([0, 0, 0, 0, 0, 0])),
        }
    }

    fn record(self: &Arc<Self>) -> Option<Error> {
        let current_thread = Arc::clone(self);

        let state = match thread::Builder::new().name(String::from("Recorder")).spawn(move || {
            *current_thread.recorder.lock().unwrap() = Some(thread::current());

            let audio_spec = WavSpec {
                channels: 2,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: SampleFormat::Float,
            };

            let taken_names = match File::search("./", "wav") {
                Ok(File::Names(value)) => value,
                Err(_) => vec![String::from("Couldn't read files")],
            };

            let mut fallbacks = 0;
            for name in &taken_names {
                if (*name).contains(&String::from("Default taken...")) {
                    fallbacks += 1;
                }
            }

            let recording_amount = taken_names.len();

            let mut new_name = String::new();

            if recording_amount > 0 {
                for item in 0..recording_amount {
                    let potential = format!("Recording {}", recording_amount + 1);
                    if potential != taken_names[item] {
                        new_name = format!("{}.wav", potential);
                    } else {
                        new_name = format!("Default taken... {}.wav", fallbacks + 1);
                        break;
                    }
                }
            } else {
                new_name = String::from("Recording 1.wav");
            }

            let mut writer = match WavWriter::create(new_name, audio_spec) {
                Ok(value) => value,
                Err(_) => {
                    return Some(Error::WriteError);
                }
            };

            let mut initial_silence = true;
            
            let record_edit = move |data: RUBuffers| {
                let mut interleaved = vec![];

                let channel1_len = data[0].len();
                let channel2_len = data[1].len();

                for sample in 0..(if channel1_len > channel2_len {
                    channel2_len
                } else {
                    channel1_len
                }) {
                    if initial_silence {
                        if data[0][sample] != 0.0 || data[1][sample] != 0.0 {
                            initial_silence = false;
                            continue;
                        } else {
                            continue;
                        }
                    } else {
                        interleaved.push(data[0][sample]);
                        interleaved.push(data[1][sample]);
                    }
                }

                if !initial_silence {
                    for sample in &interleaved {
                        writer.write_sample(*sample).unwrap();
                    }
                }
            };
            
            let callback = rucallback!(record_edit);
            
            let mut recorder = RUHear::new(callback);

            match recorder.start() {
                Ok(_) => {
                },
                Err(_) => {
                    return Some(Error::RecordError);
                },
            };

            thread::park();

            match recorder.stop() {
                Ok(_) => {
                },
                Err(_) => {
                    return Some(Error::RecordError);
                },
            };

            return None;
        }) {
            Ok(_) => None,
            Err(_) => Some(Error::RecordError),
        };

        state
    }

    fn stop(self: &Arc<Self>) {
        let current_thread = Arc::clone(self);

        if let Some(recorder) = self.recorder.lock().unwrap().as_ref() {
            recorder.unpark();
        }

        *current_thread.recorder.lock().unwrap() = None;
    }

    fn set_playing(set: Arc<RwLock<bool>>, from: bool) {
        let should_play = set;
        let mut playing = should_play.write().unwrap();
        *playing = from;
    }
}

// -------- Functions --------
fn save(data: DataType, file: &str) -> Option<Error> {
    match data {
        DataType::Settings(value) => {
            match save_file(format!("{}.bin", file), 0, &value) {
                Ok(_) => {
                    return None;
                },
                Err(_) => {
                    return Some(Error::SaveError);
                },
            }
        },
        DataType::SnapShot(value) => {
            match save_file(format!("{}.bin", file), 0, &value) {
                Ok(_) => {
                    return None;
                },
                Err(_) => {
                    return Some(Error::SaveError);
                },
            }
        },
    }
}

fn load(file: &str, kind: LoadType) -> Result<DataType, Error> {
    match kind {
        LoadType::Settings => {
            match load_file(format!("{}.bin", file), 0) {
                Ok(value) => {
                    return Ok(DataType::Settings(value));
                },
                Err(_) => {
                    return Err(Error::LoadError);
                },
            }
        },
        LoadType::Snapshot => {
            match load_file(format!("{}.bin", file), 0) {
                Ok(value) => {
                    return Ok(DataType::SnapShot(value));
                },
                Err(_) => {
                    return Err(Error::LoadError);
                },
            }
        },
    }
}

fn main() -> Result<(), Box<dyn STDError>> {
    let ui = AppWindow::new()?;

    let mut setup_error = None;

    // Creates a variable that can be used across threads and move blocks and can be read from without locking
    let tracker = Arc::new(Tracker::new(match load("settings", LoadType::Settings) {
        Ok(DataType::Settings(value)) => value,
        Ok(DataType::SnapShot(_)) => {
            setup_error = Some(Error::LoadError);
            match save(DataType::Settings(Settings::new()), "settings") {
                Some(error) => {
                    setup_error = Some(error)
                },
                None => {
                }
            };
            Settings::new()
        }
        Err(error) => {
            setup_error = Some(error);
            match save(DataType::Settings(Settings::new()), "settings") {
                Some(error) => {
                    setup_error = Some(error)
                },
                None => {
                }
            };
            Settings::new()
        }
    }));

    ui.on_update({
        let ui_handle = ui.as_weak();

        let startup_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            match setup_error {
                Some(error) => {
                    ui.set_error_notification(error.get_text());
                    ui.set_error_recieved(true);
                    setup_error = None;
                },
                None => {
                }
            };

            if ui.get_started() {
                // Acquires write access to the loaded data
                let mut settings = startup_ref_count.write().unwrap();

                settings.sync(&ui);
            }

            // Aquires read access to the loaded data
            let settings = startup_ref_count.read().unwrap();

            let index_data = settings.get_index_data();

            // Sends a list of preset names to the ui to be displayed
            ui.set_preset_names(Preset::send_names(&settings.presets, &index_data.preset_length));

            // Sends a nested list of preset values to the ui to be displayed
            ui.set_preset_values(Preset::send_values(&settings.presets, &index_data.preset_length));

            // Sends recording names to the ui to be displayed
            ui.set_recording_names(Recording::send_names(&settings.recordings));

            // Sends recording values to the ui to be displayed
            if !ui.get_locked() {
                ui.set_recording_values(Recording::send_values(&settings.recordings, &index_data.recording_length));
            }
        }
    });

    ui.on_save({
        let ui_handle = ui.as_weak();

        let update_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            // This block is used to drop the write lock on the stored data as soon as the last write is completed
            // This frees it to be used in the function called underneath and in any threads where it is needed
            {
                // Acquires write access to the loaded data
                let mut settings = update_ref_count.write().unwrap();
                settings.sync(&ui);
            }

            ui.invoke_update();

            // Aquires read access to the loaded data
            let settings = update_ref_count.read().unwrap();
            if !ui.get_locked() {
                match save(DataType::Settings((*settings).clone()), "settings") {
                    Some(error) => {
                        ui.set_error_notification(error.get_text());
                        ui.set_error_recieved(true);
                    },
                    None => {
                    }
                }
            }
        }
    });

    ui.on_record({
        let ui_handle = ui.as_weak();

        let tracker_ref_count = Arc::clone(&tracker);
        let poison_count = tracker.clone();

        move || {
            let ui = ui_handle.unwrap();

            // SnapShot::update_ui(

            if ui.get_recording() {
                match tracker_ref_count.record() {
                    Some(error) => {
                        ui.set_error_notification(error.get_text());
                        ui.set_error_recieved(true);
                        poison_count.recorder.clear_poison();
                        ui.set_recording(false);
                        tracker_ref_count.stop();
                    },
                    None => (),
                }
            } else {
                tracker_ref_count.stop();
                ui.invoke_save();
            }
        }
    });

    ui.on_delete_recordings({
        let ui_handle = ui.as_weak();

        move || {
            let ui = ui_handle.unwrap();

            match File::delete(String::from(ui.get_deleted_recording_value())) {
                Some(error) => {
                    ui.set_error_notification(error.get_text());
                    ui.set_error_recieved(true);
                },
                None => {
                },
            };

            ui.invoke_save();
        }
    });

    ui.on_play_pause({
        let ui_handle = ui.as_weak();

        let playing = tracker.playing.clone();

        let settings = tracker.settings.clone();

        let dials = tracker.snapshot_frame_values.clone();

        move || {
            let ui = ui_handle.unwrap();

            let file = String::from(ui.get_recording_names().row_data(ui.get_current_recording() as usize).unwrap());

            if ui.get_playing() || ui.get_snap_playing() {

                let values = settings.read().unwrap();
                let snapshot = if ui.get_snapping() {
                    SnapShot::new()
                } else {
                    match load(&values.recordings[ui.get_current_recording() as usize].name, LoadType::Snapshot) {
                        Ok(DataType::SnapShot(data)) => data,
                        _ => {
                            ui.set_error_notification(Error::get_text(Error::LoadError));
                            ui.set_error_recieved(true);
                            ui.set_playing(false);
                            return;
                        },
                    }
                };

                {
                    let mut should_play = playing.write().unwrap();
                    *should_play = true;
                }
                {
                    let mut dial_values = dials.write().unwrap();
                    *dial_values = Recording::parse(&values.recordings[ui.get_current_recording() as usize]);
                }

                match File::play(format!("{}.wav", file), settings.clone(), ui.get_current_recording() as usize, playing.clone(), ui.get_snapping(), ui.get_snap_playing(), snapshot, dials.clone()) {
                    Some(error) => {
                        ui.set_error_notification(error.get_text());
                        ui.set_error_recieved(true);
                        ui.set_playing(false);
                        {
                            let mut should_play = playing.write().unwrap();
                            *should_play = false;
                        }
                    },
                    None => {
                    },
                }
            } else {
                File::stop(playing.clone());
                ui.set_can_skip(true);
            }
        }
    });

    ui.on_sync_playing_with_ui({
        let ui_handle = ui.as_weak();
        let playing_ref_count = tracker.playing.clone();
        let settings_ref_count = tracker.settings.clone();
        move || {
            let ui = ui_handle.unwrap();
 
            Tracker::set_playing(playing_ref_count.clone(), if ui.get_playing() || ui.get_snap_playing() {
                true
            } else {
                let settings = settings_ref_count.read().unwrap();
                if settings.recordings.len() > 0 {
                    ui.set_dial_values(ModelRc::new(VecModel::from(settings.recordings[ui.get_current_recording() as usize].parse_vec_from_recording())));
                }
                false
            });
        }
    });

    ui.on_sync_playing_with_backend({
        let ui_handle = ui.as_weak();

        let playing = tracker.playing.clone();

        move || {
            let ui = ui_handle.unwrap();

            let is_playing = *playing.read().unwrap();

            if !is_playing {
                ui.set_backend_synced(true);
            }
        }
    });

    ui.on_snapshot_dial_update({
        let ui_handle = ui.as_weak();

        let dials = tracker.snapshot_frame_values.clone();

        move || {
            let ui = ui_handle.unwrap();

            let dial_values = dials.read().unwrap();

            ui.set_dial_values(ModelRc::new(VecModel::from(Recording::parse_vec_from_list(*dial_values))));
        }
    });

    ui.run()?;

    Ok(())
}
