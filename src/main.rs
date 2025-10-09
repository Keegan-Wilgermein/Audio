// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// -------- Imports --------
use std::{cmp::Ordering, env, error::Error as STDError, ffi::OsString, fs::{self, remove_file, rename}, sync::{Arc, Mutex, RwLock, mpsc}, thread::{self}, time::{Duration, Instant}};
use savefile::{load_file, save_file};
use savefile_derive::Savefile;
use slint::{Model, ModelRc, SharedString, ToSharedString, VecModel};
use qruhear::{RUHear, RUBuffers, rucallback};
use hound::{WavWriter, SampleFormat, WavSpec};
use kira::{effect::{eq_filter::{EqFilterBuilder, EqFilterKind}, panning_control::PanningControlBuilder}, sound::static_sound::StaticSoundData, track::{TrackBuilder}, AudioManager, AudioManagerSettings, DefaultBackend, Tween};
use rand::random_range;

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
    ShuffleError,
    DirectoryError,
    RecorderThreadError,
    PlayerThreadError,
    MessageError,
    EmptyRecordingError,
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
            Error::FallbackError => SharedString::from("Name can't contain 'Default taken...'"),
            Error::EmptyError => SharedString::from("Name has to contain something"),
            Error::ExistsError => SharedString::from("Name already exists"),
            Error::SaveFileRenameError => SharedString::from("Can't rename to 'settings'"),
            Error::PlaybackError => SharedString::from("Failed to play audio"),
            Error::ShuffleError => SharedString::from("At least three recordings required to shuffle"),
            Error::DirectoryError => SharedString::from("Couldn't find correct file directory"),
            Error::RecorderThreadError => SharedString::from("Recording thread crashed ... Restart required"),
            Error::PlayerThreadError => SharedString::from("Audio thread crashed ... Restart required"),
            Error::MessageError => SharedString::from("Incorrect message sent to thread"),
            Error::EmptyRecordingError => SharedString::from("Failed to delete new empty recording"),
        }
    }

    fn send(self, ui: &AppWindow) {
        ui.set_error_notification(self.get_text());
        ui.set_error_recieved(true);
    }
}

// Holds values used when sorting
#[derive(PartialEq)]
enum TextNum {
    Text(String),
    Number(i32),
}

impl TextNum {
    fn split_text_and_numbers(input: String) -> Vec<TextNum> {
        let mut text = String::new();
        let mut number = String::new();
        let mut list = vec![];

        let mut adding_text = false;
        let mut adding_number = false;

        let mut index = 0;
        for char in input.chars() {
            match char.to_string().parse::<i32>() {
                Ok(_) => {
                    adding_number = true;
                    number.push(char);
                    if adding_text {
                        adding_text = false;
                        if text.len() > 0 {
                            list.push(TextNum::Text(text.clone()));
                            text.clear();
                        }
                    }
                    if index == input.len() - 1 {
                        list.push(TextNum::Number(number.parse().unwrap()));
                    }
                },
                Err(_) => {
                    adding_text = true;
                    text.push(char);
                    if adding_number {
                        adding_number = false;
                        if number.len() > 0 {
                            list.push(TextNum::Number(number.parse().unwrap()));
                            number.clear();
                        }
                    }
                    if index == input.len() - 1 {
                        list.push(TextNum::Text(text.clone()));
                    }
                }
            }
            index += 1;
        }

        list
    }
}

// Types of playback
#[derive(PartialEq)]
enum Playback {
    Input(SnapShot),
    Capture(SnapShot),
    Generic(SnapShot),
}

// Mpsc messages
enum Message {
    File(String),
    PlayAudio((Playback, usize)),
    StopAudio,
    StartRecording,
    StopRecording,
}

// File stuff
#[derive(PartialEq)]
enum File {
    Names(Vec<String>),
}

impl File {
    fn search(path: &str, extension: &str, ordered: bool) -> Result<File, Error> {
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
                                                File::truncate(&mut value, ".", 0)
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

                if ordered {
                    names.sort_by(|string1, string2| {
                        let compare1 = TextNum::split_text_and_numbers(string1.to_string().to_lowercase());
                        let compare2 = TextNum::split_text_and_numbers(string2.to_string().to_lowercase());
                        let mut bias1 = 0;
                        let mut bias2 = 0;
    
                        for item in 0..if compare1.len() <= compare2.len() {
                            compare1.len()
                        } else {
                            compare2.len()
                        } {
                            if let (TextNum::Text(_), TextNum::Number(_)) = (&compare1[item], &compare2[item]) {
                                bias1 = i32::MAX;
                                break;
                            } else if let (TextNum::Number(_), TextNum::Text(_)) = (&compare1[item], &compare2[item]) {
                                bias2 = i32::MAX;
                                break;
                            } else if let (TextNum::Text(first), TextNum::Text(second)) = (&compare1[item], &compare2[item]) {
                                let first_chars: Vec<char> = first.chars().collect();
                                let second_chars: Vec<char> = second.chars().collect();
                                for char in 0..if first.len() <= second.len() {
                                    if first.len() < second.len() {
                                        bias1 += 1;
                                    }
                                    first.len()
                                } else {
                                    bias2 += 1;
                                    second.len()
                                } {
                                    match first_chars[char].cmp(&second_chars[char]) {
                                        Ordering::Greater => {
                                            bias1 += 1;
                                        },
                                        Ordering::Equal => {
                                        },
                                        Ordering::Less =>  {
                                            bias2 += 1;
                                        },
                                    }
                                }
                            } else if let (TextNum::Number(first), TextNum::Number(second)) = (&compare1[item], &compare2[item]) {
                                match first.cmp(&second) {
                                    Ordering::Greater => {
                                        bias1 += 1;
                                    },
                                    Ordering::Equal => {
                                    },
                                    Ordering::Less => {
                                        bias2 += 1;
                                    }
                                }
                            }
                        }

                        if bias1 > bias2 {
                            Ordering::Greater
                        } else if bias1 < bias2 {
                            Ordering::Less
                        } else {
                            Ordering::Equal
                        }
                    });
                }
                Ok(File::Names(names))
            },
            Err(_) => Err(Error::ReadError),
        }
    }

    fn truncate(name: &mut String, stop_char: &str, pass: u32) -> String {
        let mut length = name.len() - 1;
        let mut found = 0;
        loop {
            if name.ends_with(stop_char) {
                name.remove(length);
                length -= 1;
                if found == pass {
                    break;
                }
                found += 1;
            } else {
                if length == 1 {
                    *name = String::from("Invalid file extension");
                    break;
                }
                name.remove(length);
                length -= 1;
            }
        }

        name.to_string()
    }

    fn rename(old: &String, name: String) -> Option<Error> {
        let path = match File::get_directory() {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        match rename(format!("{}/{}.wav", path, old), format!("{}/{}.wav", path, name)) {
            Ok(_) => (),
            Err(_) => {
                return Some(Error::RenameError);
            },
        };

        match rename(format!("{}/{}.bin", path, old), format!("{}/{}.bin", path, name)) {
            Ok(_) => (),
            Err(_) => {
                return Some(Error::RenameError);
            },
        };

        None
    }

    fn delete(name: String) -> Option<Error> {
        let path = match File::get_directory() {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        match remove_file(format!("{}/{}.wav", path, name)) {
            Ok(_) => (),
            Err(_) => {
                return Some(Error::DeleteError);
            },
        };
        match remove_file(format!("{}/{}.bin", path, name)) {
            Ok(_) => None,
            Err(_) => None,
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

    fn get_directory() -> Result<String, Error> {
        let mut error = None;
        let mut string = String::new();
        match env::current_exe() {
            Ok(value) => {
                let mut name = match value.into_os_string().into_string() {
                    Ok(value) => value,
                    Err(_) => {
                        error = Some(Error::DirectoryError);
                        string
                    }
                };
                string = File::truncate(&mut name, "/", 2);
            },
            Err(_) => {
                error = Some(Error::DirectoryError);
            },
        };

        match error {
            Some(value) => Err(value),
            None => Ok(string),
        }
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
#[derive(Savefile, Clone, PartialEq)]
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

    fn shuffle(length: usize) -> Vec<i32> {
        let mut new = vec![];
        let mut avaliable = vec![];

        for number in 0..length {
            avaliable.push(number);
        }

        for _ in 0..length {
            let random = random_range(0..avaliable.len());
            new.push(avaliable[random] as i32);
            avaliable.remove(random);
        }

        new
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
        if self.recordings.len() > 0 {
            for index in 0..6 {
                dials[index] = ui.get_current_dial_values().row_data(index).unwrap();
            }
        }

        // Check for new preset creation
        if ui.get_new_preset_created() {
            self.presets.push(Preset::from(dials));
        }

        // Check for preset deletion
        if ui.get_preset_deleted() {
            if self.presets.len() > ui.get_deleted_preset_index() as usize {
                self.presets.remove(ui.get_deleted_preset_index() as usize);
                ui.set_can_delete(true);
            }
        }

        // Check for preset rename
        if ui.get_preset_renamed() {
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
        if ui.get_recording_deleted() {
            self.recordings.remove(ui.get_deleted_recording_index() as usize);
            ui.set_can_delete(true);
        }

        // Check for recording renaming
        if ui.get_recording_renamed() {
            self.recordings = match Recording::rename(&self.recordings, ui.get_recording_names()) {
                Ok(value) => value,
                Err(error) => {
                    error.1.send(ui);
                    error.0
                },
            };
        }

        // Sync recording data with any changes that might have been made to the application files
        let path = match File::get_directory() {
            Ok(value) => value,
            Err(error) => {
                error.send(ui);
                String::new()
            },
        };
        let file_names = match File::search(&path, "wav", true) {
            Ok(File::Names(value)) => value,
            Err(error) => {
                error.send(ui);
                vec![String::from("Couldn't read files")]
            }
        };

        let mut snapshot_names = match File::search(&path, "bin", true) {
            Ok(File::Names(value)) => value,
            Err(error) => {
                error.send(ui);
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
                                error.send(ui);
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
                        error.send(ui);
                    },
                    None => (),
                }
            }
        }
        
        self.recordings = updated_recordings;
    }
}

// Keeps track of the settings, the recording thread, whether recordings are being played, and the values of the dials during a set of audio frames
struct Tracker {
    settings: Arc<RwLock<Settings>>,
    locked: Arc<RwLock<Recording>>,
    playing: Arc<RwLock<bool>>,
    snapshot_frame_values: Arc<RwLock<[i32; 6]>>,
    empty_recording: Arc<RwLock<bool>>,
    recording_check: Arc<RwLock<bool>>,
}

impl Tracker {
    fn new(settings: Settings) -> Tracker {
        Tracker {
            settings: Arc::new(RwLock::new(settings)),
            locked: Arc::new(RwLock::new(Recording::new(&String::new()))),
            playing: Arc::new(RwLock::new(false)),
            snapshot_frame_values: Arc::new(RwLock::new([0, 0, 0, 0, 0, 0])),
            empty_recording: Arc::new(RwLock::new(true)),
            recording_check: Arc::new(RwLock::new(false)),
        }
    }

    fn write<T>(handle: Arc<RwLock<T>>, set: T) {
        let mut writer = handle.write().unwrap();
        *writer = set;
    }

    fn read<T: Copy>(handle: Arc<RwLock<T>>) -> T {
        let reader = handle.read().unwrap();
        *reader
    }
}

// -------- Functions --------
fn save(data: DataType, file: &str) -> Option<Error> {
    let path = match File::get_directory() {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    match data {
        DataType::Settings(value) => {
            match save_file(format!("{}/{}.bin", path, file), 0, &value) {
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
                Err(error) => {
                    println!("{}", error);
                    return Some(Error::SaveError);
                },
            }
        },
    }
}

fn load(file: &str, kind: LoadType) -> Result<DataType, Error> {
    let path = match File::get_directory() {
        Ok(value) => value,
        Err(error) => return Err(error),
    };
    match kind {
        LoadType::Settings => {
            match load_file(format!("{}/{}.bin", path, file), 0) {
                Ok(value) => {
                    return Ok(DataType::Settings(value));
                },
                Err(_) => {
                    return Err(Error::LoadError);
                },
            }
        },
        LoadType::Snapshot => {
            match load_file(format!("{}/{}.bin", path, file), 0) {
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

    let errors = Arc::new(RwLock::new(None));

    // Creates a variable that can be used across threads and move blocks and can be read from without locking
    let tracker = Arc::new(Tracker::new(match load("settings", LoadType::Settings) {
        Ok(DataType::Settings(value)) => value,
        Ok(DataType::SnapShot(_)) => {
            Tracker::write(errors.clone(), Some(Error::LoadError));
            match save(DataType::Settings(Settings::new()), "settings") {
                Some(error) => {
                    Tracker::write(errors.clone(), Some(error));
                },
                None => {
                }
            };
            Settings::new()
        }
        Err(error) => {
            Tracker::write(errors.clone(), Some(error));
            match save(DataType::Settings(Settings::new()), "settings") {
                Some(error) => {
                    Tracker::write(errors.clone(), Some(error));
                },
                None => {
                }
            };
            Settings::new()
        }
    }));

    let (record_sender, record_receiver) = mpsc::channel::<Message>();

    let record_error_handle = errors.clone();
    let recording_empty_handle = tracker.empty_recording.clone();
    let check = tracker.recording_check.clone();
    match thread::Builder::new().name(String::from("Recorder")).spawn(move || {

        let audio_spec = WavSpec {
            channels: 2,
            sample_rate: 48000,
            bits_per_sample: 32,
            sample_format: SampleFormat::Float,
        };

        let path = match File::get_directory() {
            Ok(value) => value,
            Err(_) => {
                Tracker::write(record_error_handle.clone(), Some(Error::DirectoryError));
                String::new()
            },
        };

        let empty = recording_empty_handle.clone();
        loop {
            match record_receiver.recv() {
                Ok(Message::StartRecording) => (),
                _ => {
                    Tracker::write(record_error_handle.clone(), Some(Error::MessageError));
                    continue;
                }
            }

            Tracker::write(empty.clone(), true);
            Tracker::write(check.clone(), true);

            let taken_names = match File::search(&path, "wav", false) {
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
                let potential = format!("Recording {}", recording_amount + 1);
                for item in 0..recording_amount {
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

            let mut writer = match WavWriter::create(format!("{}/{}", path, new_name), audio_spec) {
                Ok(value) => value,
                Err(_) => {
                    Tracker::write(record_error_handle.clone(), Some(Error::WriteError));
                    continue;
                }
            };

            let mut initial_silence = true;

            let empty2 = empty.clone();
            let record_callback = move |data: RUBuffers| {
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
                            Tracker::write(empty2.clone(), false);
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
            
            let callback = rucallback!(record_callback);
            
            let mut recorder = RUHear::new(callback);

            match recorder.start() {
                Ok(_) => {
                },
                Err(_) => {
                    Tracker::write(record_error_handle.clone(), Some(Error::RecordError));
                    continue;
                },
            };

            loop {
                match record_receiver.recv() {
                    Ok(Message::StopRecording) => break,
                    _ => {
                        Tracker::write(record_error_handle.clone(), Some(Error::MessageError));
                        continue;
                    }
                }
            }

            match recorder.stop() {
                Ok(_) => {
                },
                Err(_) => {
                    Tracker::write(record_error_handle.clone(), Some(Error::RecordError));
                    continue;
                },
            };

            if Tracker::read(empty.clone()) {
                match File::delete(File::truncate(&mut new_name, ".", 0)) {
                    Some(_) => {
                        Tracker::write(record_error_handle.clone(), Some(Error::EmptyRecordingError));
                    },
                    None => (),
                }
            }
        }
    }) {
        Ok(_) => (),
        Err(_) => {
            Tracker::write(errors.clone(), Some(Error::RecorderThreadError));
        }
    };

    let (audio_sender, audio_receiver) = mpsc::channel::<Message>();

    let player_error_handle = errors.clone();
    let player_settings_handle = tracker.settings.clone();
    let player_frame_handle = tracker.snapshot_frame_values.clone();
    let player_finished = tracker.playing.clone();
    match thread::Builder::new().name(String::from("Player")).spawn(move || {

        let mut sound_data;

        let mut length;

        let mut file;

        'one: loop {
            match audio_receiver.recv() {
                Ok(Message::File(name)) => {
                    file = name;
                    sound_data = match StaticSoundData::from_file(&file) {
                        Ok(value) => {
                            length = value.duration();
                            value
                        },
                        Err(_) => {
                            Tracker::write(player_error_handle.clone(), Some(Error::ReadError));
                            continue 'one;
                        }
                    };
                },
                _ => {
                    Tracker::write(player_error_handle.clone(), Some(Error::MessageError));
                    continue 'one;
                },
            }

            'two: loop {
                match audio_receiver.recv() {
                    Ok(Message::File(_)) => break 'two,
                    Ok(Message::PlayAudio(mut playback)) => {
                        let mut audio_manager = match AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()) {
                            Ok(value) => value,
                            Err(_) => {
                                Tracker::write(player_error_handle.clone(), Some(Error::PlaybackError));
                                continue 'two;
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
                                Tracker::write(player_error_handle.clone(), Some(Error::PlaybackError));
                                continue 'two;
                            }
                        };

                        let _ = match track.play(sound_data.clone()) {
                            Ok(value) => value,
                            Err(_) => {
                                Tracker::write(player_error_handle.clone(), Some(Error::PlaybackError));
                                continue 'two;
                            }
                        };
                        
                        let start = Instant::now();
                        let mut frame: usize = 0;
                        let mut previous_frame = [0, 0, 0, 0, 0, 0];
                        let mut edited_frame: usize = 0;
                        let mut capturing = false;
                        let mut snapshot = if let Playback::Capture(ref data) = playback.0 {
                            capturing = true;
                            data.clone()
                        } else if let Playback::Input(ref data) = playback.0 {
                            data.clone()
                        } else if let Playback::Generic(ref data) = playback.0 {
                            data.clone()
                        } else {
                            SnapShot::new()
                        };
                        while start.elapsed() < length {
                            match audio_receiver.try_recv() {
                                Ok(Message::StopAudio) => {
                                    if capturing {
                                        snapshot.frames.remove(0);
                                        match snapshot.save(&File::truncate(&mut file, ".", 0)) {
                                            Some(error) => {
                                                Tracker::write(player_error_handle.clone(), Some(error));
                                            },
                                            None => (),
                                        };
                                    }
                                    continue 'two;
                                },
                                Ok(Message::File(_)) => {
                                    if capturing {
                                        snapshot.frames.remove(0);
                                        match snapshot.save(&File::truncate(&mut file, ".", 0)) {
                                            Some(error) => {
                                                Tracker::write(player_error_handle.clone(), Some(error));
                                            },
                                            None => (),
                                        };
                                    }
                                    break 'two;
                                },
                                Ok(Message::PlayAudio((Playback::Capture(_), _))) => {
                                    if capturing {
                                        snapshot.frames.remove(0);
                                        match snapshot.save(&File::truncate(&mut file, ".", 0)) {
                                            Some(error) => {
                                                Tracker::write(player_error_handle.clone(), Some(error));
                                            },
                                            None => (),
                                        };
                                    }
                                    continue 'two;
                                },
                                Ok(Message::PlayAudio((value, _))) => {
                                    playback.0 = value;
                                    if let Playback::Input(ref frames) = playback.0 {
                                        snapshot = frames.clone();
                                    }
                                }
                                _ => (),
                            }
                            if let Playback::Input(_) = playback.0 {
                                if edited_frame < snapshot.frames.len() {
                                    if frame == snapshot.frames[edited_frame].1 as usize {
                                        let mut dial_values = player_frame_handle.write().unwrap();
                                        *dial_values = snapshot.frames[edited_frame].0;
                                        drop(dial_values);
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
                                    }
                                }

                            } else {
                                let settings = player_settings_handle.read().unwrap();

                                if let Playback::Capture(_) = playback.0 {
                                    if !capturing {
                                        capturing = true;
                                    }
                                    if SnapShot::edited(previous_frame, Recording::parse(&settings.recordings[playback.1])) {
                                        snapshot.frames.push((Recording::parse(&settings.recordings[playback.1]), frame as i32));
                                        previous_frame = snapshot.frames[edited_frame].0;
                                        edited_frame += 1;
                                    }
                                }
                                
                                sub_bass_handle.set_gain(if settings.recordings[playback.1].sub_bass == -7 {
                                    -60.0
                                } else {
                                    settings.recordings[playback.1].sub_bass as f32 * 4.0
                                }, Tween::default());
                                bass_handle.set_gain(if settings.recordings[playback.1].bass == -7 {
                                    -60.0
                                } else {
                                    settings.recordings[playback.1].bass as f32 * 4.0
                                }, Tween::default());
                                low_mids_handle.set_gain(if settings.recordings[playback.1].low_mids == -7 {
                                    -60.0
                                } else {
                                    settings.recordings[playback.1].low_mids as f32 * 4.0
                                }, Tween::default());
                                high_mids_handle.set_gain(if settings.recordings[playback.1].high_mids == -7 {
                                    -60.0
                                } else {
                                    settings.recordings[playback.1].high_mids as f32 * 4.0
                                }, Tween::default());
                                treble_handle.set_gain(if settings.recordings[playback.1].treble == -7 {
                                    -60.0
                                } else {
                                    settings.recordings[playback.1].treble as f32 * 4.0
                                }, Tween::default());
                                panning_handle.set_panning(settings.recordings[playback.1].pan as f32 * 0.15, Tween::default());
                                
                                drop(settings);
                            }

                            if !capturing {
                                if frame == snapshot.frames[if edited_frame < snapshot.frames.len() {
                                    edited_frame
                                } else {
                                    edited_frame - 1
                                }].1 as usize {
                                    edited_frame += 1;
                                }
                            }
                            frame += 1;

                            thread::sleep(Duration::from_millis(20));
                        }

                        Tracker::write(player_finished.clone(), true);

                        if capturing {
                            snapshot.frames.remove(0);
                            match snapshot.save(&File::truncate(&mut file, ".", 0)) {
                                Some(error) => {
                                    Tracker::write(player_error_handle.clone(), Some(error));
                                },
                                None => (),
                            };
                        }
                    },
                    _ => {
                        Tracker::write(player_error_handle.clone(), Some(Error::MessageError));
                        continue 'two;
                    }
                }
            }
        }
    }) {
        Ok(_) => (),
        Err(_) => {
            Tracker::write(errors.clone(), Some(Error::PlayerThreadError));
        }
    };

    ui.on_update({
        let ui_handle = ui.as_weak();

        let startup_ref_count = tracker.settings.clone();

        let error_handle = errors.clone();

        move || {
            let ui = ui_handle.unwrap();

            match Tracker::read(error_handle.clone()) {
                Some(error) => {
                    error.send(&ui);
                    Tracker::write(error_handle.clone(), None);
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

            if ui.get_current_recording() < settings.recordings.len() as i32 {
                ui.set_current_dial_values(ModelRc::new(VecModel::from(settings.recordings[ui.get_current_recording() as usize].parse_vec_from_recording())));
            }
        }
    });

    ui.on_update_locked_values({
        let ui_handle = ui.as_weak();

        let settings_handle = tracker.settings.clone();

        let locked_handle = tracker.locked.clone();

        move || {
            let ui = ui_handle.unwrap();

            let settings = settings_handle.read().unwrap();

            let mut locked = locked_handle.write().unwrap();

            if settings.recordings.len() > 0 {
                ui.set_dial_values_when_locked(Recording::send_values(&settings.recordings, &settings.get_index_data().recording_length));
                *locked = settings.recordings[ui.get_current_recording() as usize].clone();
            }
        }
    });

    ui.on_sync_with_locked_values({
        let ui_handle = ui.as_weak();

        let settings_handle = tracker.settings.clone();

        let locked_handle = tracker.locked.clone();

        move || {
            let ui = ui_handle.unwrap();

            let mut settings = settings_handle.write().unwrap();

            let locked = locked_handle.read().unwrap();

            settings.recordings[ui.get_current_recording() as usize].sub_bass = locked.sub_bass;
            settings.recordings[ui.get_current_recording() as usize].bass = locked.bass;
            settings.recordings[ui.get_current_recording() as usize].low_mids = locked.low_mids;
            settings.recordings[ui.get_current_recording() as usize].high_mids = locked.high_mids;
            settings.recordings[ui.get_current_recording() as usize].treble = locked.treble;
            settings.recordings[ui.get_current_recording() as usize].pan = locked.pan;

            ui.set_current_dial_values(ModelRc::new(VecModel::from(settings.recordings[ui.get_current_recording() as usize].parse_vec_from_recording())));
        }
    });

    ui.on_save({
        let ui_handle = ui.as_weak();

        let update_ref_count = tracker.settings.clone();

        let empty = tracker.empty_recording.clone();

        let just_recorded = tracker.recording_check.clone();

        move || {
            let ui = ui_handle.unwrap();

            if Tracker::read(empty.clone()) && Tracker::read(just_recorded.clone()) {
                Tracker::write(just_recorded.clone(), false);
                return;
            }

            // This block is used to drop the write lock on the stored data as soon as the last write is completed
            // This frees it to be used in the function called underneath and in any threads where it is needed
            {
                // Acquires write access to the loaded data
                let mut settings = update_ref_count.write().unwrap();
                settings.sync(&ui);
            }

            let _ = File::get_directory();

            ui.invoke_update();

            // Aquires read access to the loaded data
            let settings = update_ref_count.read().unwrap();
            if !ui.get_locked() {
                match save(DataType::Settings((*settings).clone()), "settings") {
                    Some(error) => {
                        error.send(&ui);
                    },
                    None => {
                    }
                }
            }
        }
    });

    ui.on_record({
        let ui_handle = ui.as_weak();

        let sender_handle = record_sender.clone();

        let error_handle = errors.clone();

        move || {
            let ui = ui_handle.unwrap();

            match sender_handle.send(if ui.get_recording() {
                ui.set_recording(false);
                Message::StopRecording
            } else {
                ui.set_recording(true);
                Message::StartRecording
            }) {
                Ok(_) => {
                    if !ui.get_recording() {
                        ui.invoke_save();
                        ui.invoke_gen_shuffle();
                    }
                },
                Err(_) => {
                    Tracker::write(error_handle.clone(), Some(Error::MessageError));
                }
            }
        }
    });

    ui.on_delete_recordings({
        let ui_handle = ui.as_weak();

        move || {
            let ui = ui_handle.unwrap();

            match File::delete(String::from(ui.get_deleted_recording_name())) {
                Some(error) => {
                    error.send(&ui);
                },
                None => {
                },
            };

            ui.invoke_save();
        }
    });

    ui.on_skip_audio({
        let ui_handle = ui.as_weak();

        let error_handle = errors.clone();

        let sender_handle = audio_sender.clone();

        let settings_handle = tracker.settings.clone();

         move || {
            let ui = ui_handle.unwrap();

            let settings = settings_handle.read().unwrap();

            let file = if settings.recordings.len() > 0 {
                settings.recordings[ui.get_current_recording() as usize].name.clone()
            } else {
                String::new()
            };

            let path = match File::get_directory() {
                Ok(value) => value,
                Err(error) => {
                    error.send(&ui);
                    String::new()
                },
            };

            let settings = settings_handle.read().unwrap();

            let snapshot_data = if settings.recordings.len() > 0 {
                match load(&settings.recordings[ui.get_current_recording() as usize].name, LoadType::Snapshot) {
                    Ok(DataType::SnapShot(data)) => data,
                    _ => {
                        Error::LoadError.send(&ui);
                        SnapShot::new()
                    },
                }
            } else {
                SnapShot::new()
            };
            
            if settings.recordings.len() > 0 {
                for _ in 0..if ui.get_starting_threads() {
                    1
                } else {
                    2
                } {
                    match sender_handle.send(Message::File(format!("{}/{}.wav", path, file))) {
                        Ok(_) => (),
                        Err(_) => {
                            Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                        }
                    }
                }
                if ui.get_audio_playback() {
                    match sender_handle.send(Message::PlayAudio((Playback::Generic(snapshot_data), ui.get_current_recording() as usize))) {
                        Ok(_) => (),
                        Err(_) => {
                            Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                        }
                    }
                } else if ui.get_input_playback() {
                    match sender_handle.send(Message::PlayAudio((Playback::Input(snapshot_data), ui.get_current_recording() as usize))) {
                        Ok(_) => (),
                        Err(_) => {
                            Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                        }
                    }
                } else if ui.get_input_recording() {
                    for _ in 0..2 {
                        let snapshot_data = SnapShot::new();
                        match sender_handle.send(Message::PlayAudio((Playback::Capture(snapshot_data), ui.get_current_recording() as usize))) {
                            Ok(_) => (),
                            Err(_) => {
                                Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                            }
                        }
                    }
                }

                ui.set_current_dial_values(ModelRc::new(VecModel::from(settings.recordings[ui.get_current_recording() as usize].parse_vec_from_recording())));
            }
         }
    });

    ui.on_play_generic({
        let ui_handle = ui.as_weak();

        let error_handle = errors.clone();

        let sender_handle = audio_sender.clone();

        let settings_handle = tracker.settings.clone();

         move || {
            let ui = ui_handle.unwrap();

            let settings = settings_handle.read().unwrap();

            let snapshot_data = match load(&settings.recordings[ui.get_current_recording() as usize].name, LoadType::Snapshot) {
                Ok(DataType::SnapShot(data)) => data,
                _ => {
                    Error::LoadError.send(&ui);
                    return;
                },
            };

            match sender_handle.send(if ui.get_audio_playback() {
                ui.set_audio_playback(false);
                ui.set_input_playback(false);
                ui.set_input_recording(false);
                Message::StopAudio
            } else {
                ui.set_audio_playback(true);
                ui.set_input_playback(false);
                ui.set_input_recording(false);
                ui.set_current_dial_values(ModelRc::new(VecModel::from(settings.recordings[ui.get_current_recording() as usize].parse_vec_from_recording())));
                Message::PlayAudio((Playback::Generic(snapshot_data), ui.get_current_recording() as usize))
            }) {
                Ok(_) => (),
                Err(_) => {
                    Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                }
            }
         }
    });

    ui.on_play_captured_inputs({
        let ui_handle = ui.as_weak();

        let settings_handle = tracker.settings.clone();

        let dials = tracker.snapshot_frame_values.clone();

        let error_handle = errors.clone();

        let sender_handle = audio_sender.clone();

        move || {
            let ui = ui_handle.unwrap();

            
            let settings = settings_handle.read().unwrap();

            let snapshot_data = match load(&settings.recordings[ui.get_current_recording() as usize].name, LoadType::Snapshot) {
                Ok(DataType::SnapShot(data)) => data,
                _ => {
                    Error::LoadError.send(&ui);
                    return;
                },
            };

            {
                let mut dial_values = dials.write().unwrap();
                *dial_values = Recording::parse(&settings.recordings[ui.get_current_recording() as usize]);
            }

            match sender_handle.send(if ui.get_input_playback() {
                ui.set_audio_playback(false);
                ui.set_input_playback(false);
                ui.set_input_recording(false);
                Message::StopAudio
            } else {
                ui.set_input_playback(true);
                ui.set_audio_playback(false);
                ui.set_input_recording(false);
                Message::PlayAudio((Playback::Input(snapshot_data), ui.get_current_recording() as usize))
            }) {
                Ok(_) => (),
                Err(_) => {
                    Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                }
            }
        }
    });

    ui.on_capture_inputs({
        let ui_handle = ui.as_weak();

        let error_handle = errors.clone();

        let sender_handle = audio_sender.clone();

        move || {
            let ui = ui_handle.unwrap();

            let snapshot_data = SnapShot::new();

            match sender_handle.send(if ui.get_input_playback() {
                ui.set_input_recording(false);
                ui.set_audio_playback(false);
                ui.set_input_playback(false);
                Message::StopAudio
            } else {
                ui.set_input_recording(true);
                ui.set_audio_playback(false);
                ui.set_input_playback(false);
                Message::PlayAudio((Playback::Capture(snapshot_data), ui.get_current_recording() as usize))
            }) {
                Ok(_) => (),
                Err(_) => {
                    Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                }
            }
        }
    });

    ui.on_sync_playing_with_backend({
        let ui_handle = ui.as_weak();

        let finished = tracker.playing.clone();

        let sender_handle = audio_sender.clone();

        let settings_handle = tracker.settings.clone();

        let error_handle = errors.clone();

        move || {
            let ui = ui_handle.unwrap();

            if Tracker::read(finished.clone()) {
                let settings = settings_handle.read().unwrap();
    
                let snapshot_data = match load(&settings.recordings[ui.get_current_recording() as usize].name, LoadType::Snapshot) {
                    Ok(DataType::SnapShot(data)) => data,
                    _ => {
                        Error::LoadError.send(&ui);
                        SnapShot::new()
                    },
                };
                if ui.get_playback() == PlaybackType::None {
                    ui.set_input_recording(false);
                    ui.set_audio_playback(false);
                    ui.set_input_playback(false);
                } else if ui.get_playback() == PlaybackType::Loop {
                    match sender_handle.send(if ui.get_input_recording() {
                        ui.set_input_recording(false);
                        ui.set_audio_playback(false);
                        ui.set_input_playback(false);
                        Message::StopAudio
                    } else {
                        Message::PlayAudio((if ui.get_audio_playback() {
                        Playback::Generic(snapshot_data)
                    } else if ui.get_input_playback() {
                        Playback::Input(snapshot_data)
                    } else {
                        Playback::Generic(snapshot_data)
                    }, ui.get_current_recording() as usize))
                    }) {
                        Ok(_) => (),
                        Err(_) => {
                            Tracker::write(error_handle.clone(), Some(Error::MessageError));
                        },
                    }
                } else if ui.get_playback() == PlaybackType::AutoNext {
                    let settings = settings_handle.read().unwrap();
                    if ui.get_current_recording() == (settings.recordings.len() - 1) as i32 {
                        ui.set_current_recording(0);
                    } else {
                        ui.set_current_recording(ui.get_current_recording() + 1);
                    }
                    ui.invoke_skip_audio();
                }
                Tracker::write(finished.clone(), false);
            }
        }
    });

    ui.on_snapshot_dial_update({
        let ui_handle = ui.as_weak();

        let dials = tracker.snapshot_frame_values.clone();

        move || {
            let ui = ui_handle.unwrap();

            let dial_values = dials.read().unwrap();

            ui.set_current_dial_values(ModelRc::new(VecModel::from(Recording::parse_vec_from_list(*dial_values))));
        }
    });

    ui.on_check_for_errors({
        let ui_handle = ui.as_weak();

        let error_handle = errors.clone();

        let sender = audio_sender.clone();

        let settings_handle = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            let occured = Tracker::read(error_handle.clone());
            match occured {
                Some(error) => {
                    match error {
                        Error::MessageError => {
                            if ui.get_audio_or_input_playback() || ui.get_input_recording() {
                                let settings = settings_handle.read().unwrap();
                                let file = if settings.recordings.len() > 0 {
                                    settings.recordings[ui.get_current_recording() as usize].name.clone()
                                } else {
                                    String::new()
                                };

                                let path = match File::get_directory() {
                                    Ok(value) => value,
                                    Err(error) => {
                                        error.send(&ui);
                                        String::new()
                                    },
                                };
                                match sender.send(Message::File(format!("{}/{}.wav", path, file))) {
                                    Ok(_) => (),
                                    Err(_) => (),
                                }
                            }
                        }
                        _ => (),
                    }
                    ui.set_recording(false);
                    ui.set_audio_playback(false);
                    ui.set_input_playback(false);
                    ui.set_input_recording(false);
                    error.send(&ui);
                    Tracker::write(error_handle.clone(), None);
                },
                None => ()
            }
        }
    });

    ui.on_gen_shuffle({
        let ui_handle = ui.as_weak();

        let settings_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            let settings = settings_ref_count.read().unwrap();

            if ui.get_shuffle() {
                if settings.recordings.len() > 2 {
                    ui.set_shuffle_order(ModelRc::new(VecModel::from(Recording::shuffle(settings.recordings.len()))));
                } else {
                    Error::ShuffleError.send(&ui);
                }
            }
        }
    });

    ui.run()?;

    Ok(())
}
