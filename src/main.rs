// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// -------- Imports --------
use std::{error::Error as STDError, ffi::OsString, fs::{self, remove_file, rename}, sync::{Arc, Mutex, RwLock}, thread::{self, Thread}, time::{Duration, Instant}};
use savefile::{load_file, save_file};
use savefile_derive::Savefile;
use slint::{Model, ModelRc, SharedString, ToSharedString, VecModel};
use qruhear::{RUHear, RUBuffers, rucallback};
use hound::{WavWriter, SampleFormat, WavSpec};
use kira::{effect::{eq_filter::{EqFilterBuilder, EqFilterKind}, panning_control::PanningControlBuilder}, sound::static_sound::StaticSoundData, track::TrackBuilder, AudioManager, AudioManagerSettings, DefaultBackend, Tween};

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
    PlaybackError,
    ControllerError,
}

impl Error {
    fn get_text(kind: Error) -> SharedString {
        match kind {
            Error::SaveError => SharedString::from("Failed to save data ... Reverting to previous save"),
            Error::LoadError => SharedString::from("Data doesn't exist ... Creating new file"),
            Error::RecordError => SharedString::from("Recording failed ... Please try again"),
            Error::WriteError => SharedString::from("Failed to write audio"),
            Error::ReadError => SharedString::from("File read failed"),
            Error::RenameError => SharedString::from("Failed to rename file"),
            Error::DeleteError => SharedString::from("Failed to delete file"),
            Error::FallbackError => SharedString::from("Can't rename to fallback name"),
            Error::EmptyError => SharedString::from("Name has to contain something"),
            Error::ExistsError => SharedString::from("Name already exists"),
            Error::PlaybackError => SharedString::from("Failed to play audio"),
            Error::ControllerError => SharedString::from("Audio controller crashed ... restart required"),
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
                                            Ok(value) => {
                                                File::truncate(value)
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

    fn truncate(mut name: String) -> String {
        let mut length = name.len() - 1;
        loop {
            if name.ends_with(".") {
                name.remove(length);
                break;
            } else {
                if length == 1 {
                    name = String::from("Invalid file extension");
                }
                name.remove(length);
                length -= 1;
            }
        }

        String::from(name)
    }

    fn rename(old: &String, name: String) -> Option<Error> {
        match rename(format!("{}.wav", old), format!("{}.wav", name)) {
            Ok(_) => {
            },
            Err(_) => {
                return Some(Error::RenameError);
            },
        };

        None
    }

    fn delete(name: String) -> Option<Error> {
        match remove_file(format!("./{}.wav", name)) {
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

    fn play(file: String, values: Arc<RwLock<Settings>>, selected_recording: usize) -> Option<Error> {

        let state = match thread::Builder::new().name(String::from("Player")).spawn(move || {

            let mut audio_manager = match AudioManager::<DefaultBackend>::new(AudioManagerSettings::default()) {
                Ok(value) => value,
                Err(_) => {
                    return Some(Error::ControllerError);
                }
            };

            let sub_bass = EqFilterBuilder::new(EqFilterKind::LowShelf, 80.0, 0.0, 0.8);
            let bass = EqFilterBuilder::new(EqFilterKind::Bell, 120.0, 0.0, 0.4);
            let mids = EqFilterBuilder::new(EqFilterKind::Bell, 500.0, 0.0, 0.2);
            let vocals = EqFilterBuilder::new(EqFilterKind::Bell, 2500.0, 0.0, 0.2);
            let treble = EqFilterBuilder::new(EqFilterKind::HighShelf, 6000.0, 0.0, 0.8);
            let pan = PanningControlBuilder::default();

            let mut builder = TrackBuilder::new();
            let mut sub_bass_handle = builder.add_effect(sub_bass);
            let mut bass_handle = builder.add_effect(bass);
            let mut mids_handle = builder.add_effect(mids);
            let mut vocals_handle = builder.add_effect(vocals);
            let mut treble_handle = builder.add_effect(treble);
            let mut panning_handle = builder.add_effect(pan);

            let mut track = match audio_manager.add_sub_track(builder) {
                Ok(value) => value,
                Err(_) => {
                    return Some(Error::PlaybackError);
                }
            };

            let sound_data = match StaticSoundData::from_file(file) {
                Ok(value) => value,
                Err(_) => {
                    return Some(Error::ReadError);
                }
            };

            let length = sound_data.duration();

            let _ = match track.play(sound_data) {
                Ok(value) => value,
                Err(_) => {
                    return Some(Error::PlaybackError);
                }
            };

            let start = Instant::now();
            while start.elapsed() < length {
                {
                    let value = values.try_read().unwrap();
                    sub_bass_handle.set_gain(value.recordings[selected_recording].sub_bass as f32 * 1.5, Tween::default());
                    bass_handle.set_gain(value.recordings[selected_recording].bass as f32 * 1.5, Tween::default());
                    mids_handle.set_gain(value.recordings[selected_recording].mids as f32 * 1.5, Tween::default());
                    vocals_handle.set_gain(value.recordings[selected_recording].vocals as f32 * 1.5, Tween::default());
                    treble_handle.set_gain(value.recordings[selected_recording].treble as f32 * 1.5, Tween::default());
                    panning_handle.set_panning(value.recordings[selected_recording].pan as f32 * 0.15, Tween::default());
                }
                thread::sleep(Duration::from_millis(50)); // update every 50ms
            }

            None
        }) {
            Ok(_) => None,
            Err(_) => Some(Error::PlaybackError),
        };

        state
    }

    fn stop() {
        
    }
}

// -------- Structs --------
// Index data for Settings struct
struct IndexData {
    preset_length: usize,
    recording_length: usize,
}

// Preset data
#[derive(Savefile)]
struct Preset {
    name: String,
    sub_bass: i32,
    bass: i32,
    mids: i32,
    vocals: i32,
    treble: i32,
    pan: i32,
}

impl Preset {
    fn from(values: [i32; 6]) -> Preset {
        Preset {
            name: String::from("New Preset"),
            sub_bass: values[0],
            bass: values[1],
            mids: values[2],
            vocals: values[3],
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
            preset_values.push(list[values].mids);
            preset_values.push(list[values].vocals);
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
    mids: i32,
    vocals: i32,
    treble: i32,
    pan: i32,
}

impl Recording {
    fn new(name: String) -> Recording {
        Recording {
            name: name,
            sub_bass: 0,
            bass: 0,
            mids: 0,
            vocals: 0,
            treble: 0,
            pan: 0,
        }
    }

    fn from(name: String, values: [i32; 6]) -> Recording {
        Recording {
            name: name,
            sub_bass: values[0],
            bass: values[1],
            mids: values[2],
            vocals: values[3],
            treble: values[4],
            pan: values[5],
        }
    }

    fn parse(recording: &Recording) -> [i32; 6] {
        let mut list = [0, 0, 0, 0, 0, 0];

        list[0] = recording.sub_bass;
        list[1] = recording.bass;
        list[2] = recording.mids;
        list[3] = recording.vocals;
        list[4] = recording.treble;
        list[5] = recording.pan;

        list
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
            recording_values.push(list[values].mids);
            recording_values.push(list[values].vocals);
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
        let mut rename_failed = (false, None);

        for name in 0..old.len() {
            if new.row_data(name).unwrap() != old[name].name {
                if new.row_data(name).unwrap() == String::from("Default taken...") {
                    recording_list.push(Recording::from(old[name].name.clone(), Recording::parse(&old[name])));
                    fallback_error_occured = true;
                } else if new.row_data(name).unwrap().is_empty() || new.row_data(name).unwrap() == String::from("") {
                    recording_list.push(Recording::from(old[name].name.clone(), Recording::parse(&old[name])));
                    empty_error_occured = true;
                } else if File::exists(String::from(new.row_data(name).unwrap()), &old) {
                    recording_list.push(Recording::from(old[name].name.clone(), Recording::parse(&old[name])));
                    exists_error_occured = true;
                } else {
                    match File::rename(&old[name].name, String::from(new.row_data(name).unwrap())) {
                        Some(error) => {
                            rename_failed = (true, Some(error));
                        },
                        None => {
                        }
                    }
                    recording_list.push(Recording::from(String::from(new.row_data(name).unwrap()), Recording::parse(&old[name])));
                }
            } else {
                recording_list.push(Recording::from(old[name].name.clone(), Recording::parse(&old[name])));
            }
        }
        
        if exists_error_occured {
            Err((recording_list, Error::ExistsError))
        } else if empty_error_occured {
            Err((recording_list, Error::EmptyError))
        } else if fallback_error_occured {
            Err((recording_list, Error::FallbackError))
        } else if rename_failed.0 {
            Err((recording_list, rename_failed.1.unwrap()))
        } else {
            Ok(recording_list)
        }
    }

}

// All settings data
#[derive(Savefile)]
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

    fn sync(&mut self, new: &AppWindow) {
        let index_data = self.get_index_data();

        let mut dials = [0, 0, 0, 0, 0, 0];
        for index in 0..6 {
            dials[index] = new.get_dial_values().row_data(index).unwrap();
        }

        // Check for new preset creation
        if new.get_new_preset() {
            self.presets.push(Preset::from(dials));
        }

        // Check for preset deletion
        if new.get_delete_preset() {
            self.presets.remove(new.get_deleted_preset() as usize);
        }

        // Check for preset rename
        if new.get_rename_preset() {
            for preset in 0..index_data.preset_length {
                self.presets[preset].name = String::from(match new.get_preset_names().row_data(preset) {
                    Some(name) => name,
                    None => SharedString::from("New Preset"),
                });
            }
        }

        // Check for recording edits
        if index_data.recording_length > 0 {
            let position = new.get_current_recording() as usize;
            if new.get_dials_edited() {
                self.recordings[position] = Recording::from(self.recordings[position].name.clone(), dials);
            }
        }

        // Check for recording deletion
        if new.get_delete_recording() {
            self.recordings.remove(new.get_deleted_recording() as usize);
        }

        // Check for recording renaming
        if new.get_renamed_recording() {
            self.recordings = match Recording::rename(&self.recordings, new.get_recording_names()) {
                Ok(value) => value,
                Err(error) => {
                    new.set_error_notification(Error::get_text(error.1));
                    new.set_error_recieved(true);
                    error.0
                },
            };
            self.recordings.sort_by_key(|recording| recording.name.clone());
        }

        // Sync recording data with any changes that might have been made while the app was closed
        if new.get_started() || new.get_new_recording() {
            let file_names = match File::search("./", "wav") {
                Ok(File::Names(value)) => value,
                Err(error) => {
                    new.set_error_notification(Error::get_text(error));
                    new.set_error_recieved(true);
                    vec![String::from("Couldn't read files")]
                }
            };

            let mut updated_recordings = vec![];

            for name in 0..file_names.len() {
                if self.recordings.len() > 0 {
                    for recording in 0..self.recordings.len() {
                        if self.recordings[recording].name == file_names[name] {
                            updated_recordings.push(Recording::from(file_names[name].clone(), Recording::parse(&self.recordings[recording])));
                            break;
                        }
                        if recording == self.recordings.len() - 1 {
                            updated_recordings.push(Recording::new(file_names[name].clone()));
                        }
                    }
                } else {
                    updated_recordings.push(Recording::new(file_names[name].clone()));
                }
            }

            updated_recordings.sort_by_key(|recording| recording.name.clone());
            self.recordings = updated_recordings;
        }
    }
}

// Keeps track of the settings and the recording thread
struct Tracker {
    settings: Arc<RwLock<Settings>>,
    recorder: Arc<Mutex<Option<Thread>>>,
}

impl Tracker {
    fn new(settings: Settings) -> Tracker {
        Tracker {
            settings: Arc::new(RwLock::new(settings)),
            recorder: Arc::new(Mutex::new(None)),
        }
    }

    fn record(self: &Arc<Self>) -> Option<Error> {
        let current_thread = Arc::clone(self);

        let state = match thread::Builder::new().name(String::from("Recorder")).spawn(move || {
            *current_thread.recorder.lock().unwrap() = Some(thread::current());

            let audio_spec = WavSpec {
                channels: 1,
                sample_rate: 48000,
                bits_per_sample: 32,
                sample_format: SampleFormat::Float,
            };

            let taken_names = match File::search("./", "wav") {
                Ok(File::Names(value)) => value,
                Err(_) => vec![String::from("Couldn't read files")],
            };

            let recording_amount = taken_names.len();

            let mut new_name = String::new();

            if recording_amount > 0 {
                for item in 0..recording_amount {
                    let potential = format!("Recording {}", recording_amount + 1);
                    if potential != taken_names[item] {
                        new_name = format!("{}.wav", potential);
                    } else {
                        new_name = String::from("Default taken....wav");
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
            
            let record_edit = move |data: RUBuffers| {
                for sample in &data[1] {
                    writer.write_sample(*sample).unwrap();
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
}

// -------- Functions --------
fn save(data: &Settings) -> Option<Error> {
    match save_file("settings.bin", 0, data) {
        Ok(_) => None,
        Err(_) => Some(Error::SaveError),
    }
}

fn load() -> Result<Settings, Error> {
    match load_file("settings.bin", 0) {
        Ok(value) => Ok(value),
        Err(_) => Err(Error::LoadError),
    }
}

fn main() -> Result<(), Box<dyn STDError>> {
    let ui = AppWindow::new()?;

    let mut setup_error = None;

    // Creates a variable that can be used across threads and move blocks and can be read from without locking
    let tracker = Arc::new(Tracker::new(match load() {
        Ok(value) => value,
        Err(error) => {
            let _ = save(&Settings::new());
            setup_error = Some(error);
            Settings::new()
        }
    }));

    ui.on_update({
        let ui_handle = ui.as_weak();

        let startup_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            match setup_error {
                Some(value) => {
                    ui.set_error_notification(Error::get_text(value));
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
            ui.set_recording_values(Recording::send_values(&settings.recordings, &index_data.recording_length));
        }
    });

    ui.on_save({
        let ui_handle = ui.as_weak();

        let update_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            // This block is used to drop the write lock on the stored data as soon as the last write is completed
            // This frees it to be used in the function called underneath
            {
                // Acquires write access to the loaded data
                let mut settings = update_ref_count.write().unwrap();
                settings.sync(&ui);
            }

            ui.invoke_update();

            // Aquires read access to the loaded data
            let settings = update_ref_count.read().unwrap();
            let _ = save(&settings);
        }
    });

    ui.on_record({
        let ui_handle = ui.as_weak();

        let tracker_ref_count = Arc::clone(&tracker);
        let poison_count = tracker.clone();

        move || {
            let ui = ui_handle.unwrap();

            if ui.get_recording() {
                match tracker_ref_count.record() {
                    Some(error) => {
                        ui.set_error_notification(Error::get_text(error));
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
                    ui.set_error_notification(Error::get_text(error));
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

        move || {
            let ui = ui_handle.unwrap();

            let file = String::from(ui.get_recording_names().row_data(ui.get_current_recording() as usize).unwrap());

            if ui.get_playing() {
                match File::play(format!("{}.wav", file), tracker.settings.clone(), ui.get_current_recording() as usize) {
                    Some(error) => {
                        ui.set_error_notification(Error::get_text(error));
                        ui.set_error_recieved(true);
                        ui.set_playing(false);
                    },
                    None => {
                    },
                }
            } else {
                File::stop();
            }
        }
    });

    // ui.on_name({
    //     let ui_handle = ui.as_weak();
    //     move || {
    //         let ui = ui_handle.unwrap();
    //     }
    // });

    ui.run()?;

    Ok(())
}