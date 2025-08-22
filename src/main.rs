// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// -------- Imports --------
use std::{error::Error as STDError, sync::{Arc, Mutex, RwLock}, thread::{self, Thread}};
use savefile::{load_file, save_file};
use savefile_derive::Savefile;
use slint::{Model, ModelRc, SharedString, ToSharedString};
use qruhear::{RUHear, RUBuffers, rucallback};

slint::include_modules!();

// -------- Enums --------
// Errors
#[derive(Clone, Copy, PartialEq)]
enum Error {
    SaveError,
    LoadError,
    RecordError,
}

impl Error {
    fn get_text(kind: Error) -> String {
        match kind {
            Error::SaveError => String::from("Failed to save data ... Reverting to previous save"),
            Error::LoadError => String::from("Data doesn't exist ... Creating save file"),
            Error::RecordError => String::from("Recording failed ... Please try again"),
        }
    }
}

// Successes
enum Success {
    SaveSuccess,
    RecordSuccess
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
    bass: i32,
    vocals: i32,
    treble: i32,
    gain: i32,
    reverb: i32,
    crush: i32,
}

impl Preset {
    fn from(values: [i32; 6]) -> Preset {
        Preset {
            name: String::from("New Preset"),
            bass: values[0],
            vocals: values[1],
            treble: values[2],
            gain: values[3],
            reverb: values[4],
            crush: values[5],
        }
    }

    fn get_names(list: &Vec<Preset>, length: &usize) -> ModelRc<SharedString> {
        let mut preset_names: Vec<SharedString> = vec![];
        for preset in 0..*length {
            preset_names.push(list[preset].name.to_shared_string());
        }
        ModelRc::new(slint::VecModel::from(preset_names))
    }

    fn get_values(list: &Vec<Preset>, length: &usize) -> ModelRc<ModelRc<i32>> {
        let mut all_preset_values: Vec<ModelRc<i32>> = vec![];
        for values in 0..*length {
            let mut preset_values: Vec<i32> = vec![];
            
            preset_values.push(list[values].bass);
            preset_values.push(list[values].vocals);
            preset_values.push(list[values].treble);
            preset_values.push(list[values].gain);
            preset_values.push(list[values].reverb);
            preset_values.push(list[values].crush);

            all_preset_values.push(ModelRc::new(slint::VecModel::from(preset_values)));
        }
        ModelRc::new(slint::VecModel::from(all_preset_values))
    }
}

// Recording data
#[derive(Savefile)]
struct Recording {
    name: String,
    bass: i32,
    vocals: i32,
    treble: i32,
    gain: i32,
    reverb: i32,
    crush: i32,
    data: Vec<Vec<f32>>,
}

impl Recording {
    fn new_values() -> [i32; 6] {
        [0, 0, 0, 0, 0, 0]
    }

    fn from(values: [i32; 6], data: Vec<Vec<f32>>) -> Recording {
        Recording {
            name: String::from("New recording"),
            bass: values[0],
            vocals: values[1],
            treble: values[2],
            gain: values[3],
            reverb: values[4],
            crush: values[5],
            data: data,
        }
    }

    fn get_names(list: &Vec<Recording>, length: &usize) -> ModelRc<SharedString> {
        let mut recording_names: Vec<SharedString> = vec![];
        for recording in 0..*length {
            recording_names.push(list[recording].name.to_shared_string());
        }
        ModelRc::new(slint::VecModel::from(recording_names))
    }

    fn get_values(list: &Vec<Recording>, length: &usize) -> ModelRc<ModelRc<i32>> {
        let mut all_recording_values: Vec<ModelRc<i32>> = vec![];
        for values in 0..*length {
            let mut recording_values: Vec<i32> = vec![];
            
            recording_values.push(list[values].bass);
            recording_values.push(list[values].vocals);
            recording_values.push(list[values].treble);
            recording_values.push(list[values].gain);
            recording_values.push(list[values].reverb);
            recording_values.push(list[values].crush);

            all_recording_values.push(ModelRc::new(slint::VecModel::from(recording_values)));
        }
        ModelRc::new(slint::VecModel::from(all_recording_values))
    }

    fn update_values(&mut self, values: [i32; 6]) -> Recording {
        Recording {
            name: self.name.clone(),
            bass: values[0],
            vocals: values[1],
            treble: values[2],
            gain: values[3],
            reverb: values[4],
            crush: values[5],
            data: self.data.clone(),
        }
    }

    fn edited(record: &Recording, list: [i32; 6]) -> bool {
        if record.bass == list[0]
        && record.vocals == list[1]
        && record.treble == list[2]
        && record.gain == list[3]
        && record.reverb == list[4]
        && record.crush == list[5] {
            false
        } else {
            true
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
                    None => slint::SharedString::from("New Preset"),
                });
            }
        }

        // Check for recording edits
        if index_data.recording_length > 0 {
            if Recording::edited(&self.recordings[new.get_current_recording() as usize], dials) {
                self.recordings[new.get_current_recording() as usize] = self.recordings[new.get_current_recording() as usize].update_values(dials);
            }
        }

        // Check for recording deletion
        if new.get_delete_recording() {
            self.recordings.remove(new.get_deleted_recording() as usize);
        }

        // Check for recording rename
        if new.get_rename_recording() {
            for recording in 0..index_data.recording_length {
                self.recordings[recording].name = String::from(match new.get_recording_names().row_data(recording) {
                    Some(name) => name,
                    None => slint::SharedString::from("New Recording"),
                });
            }
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

    fn record(self: &Arc<Self>) -> Result<Success, Error> {
        let current_thread = Arc::clone(self);

        let state = match thread::Builder::new().name(String::from("Recorder")).spawn(move || {
            *current_thread.recorder.lock().unwrap() = Some(thread::current());

            let mut settings = current_thread.settings.write().unwrap();

            let audio_buffer = Arc::new(Mutex::new(Vec::new()));
            let audio_buffer_clone = Arc::clone(&audio_buffer);
            
            let record_edit = move |data: RUBuffers| {
                audio_buffer_clone.lock().unwrap().push(Tracker::serialise(data, 1));
            };
            
            let callback = rucallback!(record_edit);
            
            let mut recorder = RUHear::new(callback);

            match recorder.start() {
                Ok(_) => Success::RecordSuccess,
                Err(_) => {
                    return Err(Error::RecordError);
                },
            };

            thread::park();

            settings.recordings.push(Recording::from(Recording::new_values(), audio_buffer.lock().unwrap().clone()));

            match recorder.stop() {
                Ok(_) => Success::RecordSuccess,
                Err(_) => {
                    return Err(Error::RecordError);
                },
            };

            return Ok(Success::RecordSuccess);
        }) {
            Ok(_) => Ok(Success::RecordSuccess),
            Err(_) => Err(Error::RecordError),
        };

        state
    }

    fn serialise(data: Vec<Vec<f32>>, channel: usize) -> Vec<f32> {
        data[channel].clone()
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
fn save(data: &Settings) -> Result<Success, Error> {
    match save_file("settings.bin", 0, data) {
        Ok(_) => Ok(Success::SaveSuccess),
        Err(_) => Err(Error::SaveError),
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

    let mut load_error = None;

    // Creates a variable that can be used across threads and move blocks and can be read from without locking
    let tracker = Arc::new(Tracker::new(match load() {
        Ok(value) => value,
        Err(error) => {
            let _ = save(&Settings::new());
            load_error = Some(error);
            Settings::new()
        }
    }));

    ui.on_startup({
        let ui_handle = ui.as_weak();

        let startup_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            match load_error {
                Some(value) => {
                    ui.set_error_notification(slint::SharedString::from(Error::get_text(value)));
                    ui.set_error_recieved(true);
                    load_error = None;
                },
                None => {

                }
            }

            // Acquires read access to the loaded data
            let settings = startup_ref_count.read().unwrap();

            let index_data = settings.get_index_data();

            // Sends a list of preset names to the UI to be displayed
            ui.set_preset_names(Preset::get_names(&settings.presets, &index_data.preset_length));

            // Sends a nested list of preset values to the UI to be displayed
            ui.set_preset_values(Preset::get_values(&settings.presets, &index_data.preset_length));

            // Sends a list of recording names to the UI to be displayed
            ui.set_recording_names(Recording::get_names(&settings.recordings, &index_data.recording_length));

            // Sends a nested list of recording edits to the UI to be displayed
            ui.set_recording_values(Recording::get_values(&settings.recordings, &index_data.recording_length));
        }
    });

    ui.on_update_and_save({
        let ui_handle = ui.as_weak();

        let update_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            // This block is used to drop the write lock on the stored data as soon as the last write is completed
            // This frees it to be used elsewhere slightly quicker
            {
                // Acquires write access to the loaded data
                let mut settings = update_ref_count.write().unwrap();
                settings.sync(&ui);
            }

            ui.invoke_startup();

            // Aquires read access to the loaded data
            let settings = update_ref_count.read().unwrap();
            let _ = save(&settings);
        }
    });

    ui.on_record({
        let ui_handle = ui.as_weak();

        let tracker_ref_count = Arc::clone(&tracker);

        move || {
            let ui = ui_handle.unwrap();

            if ui.get_recording() {
                match tracker_ref_count.record() {
                    Ok(_) => (),
                    Err(error) => {
                        ui.set_error_notification(slint::SharedString::from(Error::get_text(error)));
                        ui.set_error_recieved(true);
                        tracker.recorder.clear_poison();
                        ui.set_recording(false);
                        tracker_ref_count.stop();
                    }
                }
            } else {
                tracker_ref_count.stop();
            }

            ui.invoke_update_and_save();
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
