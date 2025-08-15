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
enum Error {
    SaveError,
    LoadError,
}

// Successes
enum Success {
    SaveSuccess,
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
    bass: i32,
    vocals: i32,
    treble: i32,
    gain: i32,
    reverb: i32,
    crush: i32,
}

impl Recording {
    fn new() -> Recording {
        Recording {
            bass: 0,
            vocals: 0,
            treble: 0,
            gain: 0,
            reverb: 0,
            crush: 0,
        }
    }

    fn from(values: [i32; 6]) -> Recording {
        Recording {
            bass: values[0],
            vocals: values[1],
            treble: values[2],
            gain: values[3],
            reverb: values[4],
            crush: values[5],
        }
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
            recordings: vec![Recording::new(), Recording::new()],
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

        // Check for recording edits
        if index_data.recording_length > 0 {
            if Recording::edited(&self.recordings[new.get_current_recording() as usize], dials) {
                self.recordings[new.get_current_recording() as usize] = Recording::from(dials);
            }
        }

        // Check for recording deletion
        if new.get_delete_recording() {
            self.recordings.remove(new.get_deleted_recording() as usize);
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

    fn record(self: &Arc<Self>) {
        let current_thread = Arc::clone(self);

        let _ = thread::Builder::new().name(String::from("Recorder")).spawn(move || {
            *current_thread.recorder.lock().unwrap() = Some(thread::current());

            let record_edit = |data: RUBuffers| {
                // println!("{:?}", data);
            };

            let callback = rucallback!(record_edit);

            let mut recorder = RUHear::new(callback);

            let _ = recorder.start();
            thread::park();

            let _ = recorder.stop();

            *current_thread.recorder.lock().unwrap() = None;
        });
    }

    fn stop(self: &Arc<Self>) {
        let _ = Arc::clone(self);

        if let Some(recorder) = self.recorder.lock().unwrap().as_ref() {
            recorder.unpark();
        }
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

    // Creates a variable that can be used across threads and move blocks and can be read from without locking
    let tracker = Arc::new(Tracker::new(match load() {
        Ok(value) => value,
        Err(_) => {
            let _ = save(&Settings::new());
            Settings::new()
        }
    }));

    ui.on_startup({
        let ui_handle = ui.as_weak();

        let startup_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            // Acquires read access to the loaded data
            let settings = startup_ref_count.read().unwrap();

            let index_data = settings.get_index_data();

            // Sends a list of preset names to the UI to be displayed
            ui.set_preset_names(Preset::get_names(&settings.presets, &index_data.preset_length));

            // Sends a nested list of preset values to the UI to be displayed
            ui.set_preset_values(Preset::get_values(&settings.presets, &index_data.preset_length));

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
                tracker_ref_count.record();
            } else {
                tracker_ref_count.stop();
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
