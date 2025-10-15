// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// -------- Imports --------
use hound::{SampleFormat, WavSpec, WavWriter}; // Imports for writing recorded data to disk
use kira::{
    // Imports for playing back recordings and editing them
    effect::{
        eq_filter::{EqFilterBuilder, EqFilterKind},
        panning_control::PanningControlBuilder,
    },
    sound::static_sound::StaticSoundData,
    track::TrackBuilder,
    AudioManager,
    AudioManagerSettings,
    DefaultBackend,
    Tween,
};
use qruhear::{rucallback, RUBuffers, RUHear}; // Imports for recording audio
use rand::random_range; // Random numbers
use savefile::{load_file, save_file}; // Saving settings and snapshot data
use savefile_derive::Savefile;
use slint::{Model, ModelRc, SharedString, ToSharedString, VecModel}; // Imports for UI
use std::{
    // Threads, file reading, current time, and reference variables
    cmp::Ordering,
    env,
    error::Error as STDError,
    ffi::OsString,
    fs::{self, remove_file, rename},
    sync::{mpsc, Arc, Mutex, RwLock},
    thread::{self},
    time::{Duration, Instant},
};

slint::include_modules!(); // Imports the auto generated functions used to control the UI variables

// -------- Enums --------
// Errors
#[derive(Clone, Copy, PartialEq)] // Derives attributes like .clone() and ==
enum Error {
    // Keeps track of errors
    SaveError,           // Error while saving any data
    LoadError,           // Error while loading any data
    RecordError,         // Error while recording audio
    WriteError,          // Error while saving audio data
    ReadError,           // Error while reading data on disk
    RenameError,         // Error while renaming file
    DeleteError,         // Error while deleting file
    FallbackError,       // Attempt to rename recording to 'Default taken...'
    EmptyError,          // Attempt to rename recording to ''
    ExistsError,         // Attempt to rename recording to an already existing name
    SaveFileRenameError, // Attempt to rename recording to 'settings'
    PlaybackError,       // Error playing audio
    ShuffleError,        // Not enough recordings to shuffle
    DirectoryError,      // Returned directory not the working directory
    RecorderThreadError, // Recorder thread failed to start
    PlayerThreadError,   // Player thread failed to start
    MessageError,        // Unexpected message sent to thread
    EmptyRecordingError, // Specifically when a recording is made that contains no sound and couldn't be automatically deleted
}

impl Error {
    fn get_text(self) -> SharedString {
        // Takes an error value and returns a shared string to send to the ui
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
            Error::ShuffleError => {
                SharedString::from("At least three recordings required to shuffle")
            }
            Error::DirectoryError => SharedString::from("Couldn't find correct file directory"),
            Error::RecorderThreadError => {
                SharedString::from("Recording thread crashed ... Restart required")
            }
            Error::PlayerThreadError => {
                SharedString::from("Audio thread crashed ... Restart required")
            }
            Error::MessageError => SharedString::from("Incorrect message sent to thread"),
            Error::EmptyRecordingError => {
                SharedString::from("Failed to delete new empty recording")
            }
        }
    }

    fn send(self, ui: &AppWindow) {
        // Takes an error value and updates the ui
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
        // Takes a string and returns a vector of itself
        let mut text = String::new();
        let mut number = String::new();
        let mut list = vec![];

        let mut adding_text = false; // Keeps track of whether the last character was text
        let mut adding_number = false; // Keeps track of whether the last character was a number

        let mut index = 0;
        for char in input.chars() {
            // Loops over every character in the input string
            match char.to_string().parse::<i32>() {
                // Attempts to parse the char into an i32
                Ok(_) => {
                    // If parse is successful
                    adding_number = true;
                    number.push(char); // Adds char to the numbers list
                    if adding_text {
                        adding_text = false;
                        if text.len() > 0 {
                            list.push(TextNum::Text(text.clone())); // Pushes the text string onto the final list
                            text.clear(); // Clears the text string for another use
                        }
                    }
                    if index == input.len() - 1 {
                        // Checks to see if on the last char
                        list.push(TextNum::Number(number.parse().unwrap())); // Pushes the number string onto the final list after parsing it into an i32
                    }
                }
                Err(_) => {
                    // If parse fails
                    // Do the same thing as if it was successful but with the opposite strings
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

        list // Return the final list
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
    File(String),                 // Path
    PlayAudio((Playback, usize)), // Type, index of current recording
    StopAudio,
    StartRecording,
    StopRecording,
}

// Files
#[derive(PartialEq)]
enum File {
    Names(Vec<String>),
}

impl File {
    fn search(path: &str, extension: &str, ordered: bool) -> Result<File, Error> {
        // Searches for files at the specified path and the same extension, and returns either a list of names or an error
        let mut names = vec![];
        match fs::read_dir(path) {
            // Attemps to read the files at the specified path
            Ok(directories) => {
                for entry in directories {
                    // Loop throuh every entry
                    match entry {
                        Ok(directory) => {
                            let path = directory.path(); // Get the path of each entry

                            if path.is_file() {
                                // If it's a file not a folder
                                if let Some(file_type) = path.extension() {
                                    // Gets the extension of the file
                                    if file_type == extension {
                                        // Checks if it's correct
                                        let file_name = match path.file_name() {
                                            // Gets the file name
                                            Some(value) => value.to_owned(),
                                            None => OsString::from("Couldn't read name"),
                                        };
                                        names.push(match file_name.into_string() {
                                            // Pushes the file name onto the list of names
                                            Ok(mut value) => File::truncate(&mut value, ".", 0), // Truncates the extension on the name
                                            Err(_) => String::from("Couldn't read name"),
                                        });
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            return Err(Error::ReadError);
                        }
                    }
                }

                if ordered {
                    // If true passed as the ordering value
                    names.sort_by(|string1, string2| {
                        // Sorts the names list using a custom rule set
                        let compare1 =
                            TextNum::split_text_and_numbers(string1.to_string().to_lowercase()); // Splits string into letters and whole numbers
                        let compare2 =
                            TextNum::split_text_and_numbers(string2.to_string().to_lowercase());
                        // The largest bias is sorted after the smaller one
                        let mut bias1 = 0;
                        let mut bias2 = 0;

                        for item in 0..if compare1.len() <= compare2.len() {
                            // Loops through all the items in the smallest list
                            compare1.len()
                        } else {
                            compare2.len()
                        } {
                            if let (TextNum::Text(_), TextNum::Number(_)) =
                                // Checks if the first list is text and the second is a number
                                (&compare1[item], &compare2[item])
                            {
                                bias1 = i32::MAX; // Sets bias1 to the maximum value for an i32
                                break; // Skips the rest of the checks as they no longer matter
                            } else if let (TextNum::Number(_), TextNum::Text(_)) =
                                // Does the opposite
                                (&compare1[item], &compare2[item])
                            {
                                bias2 = i32::MAX;
                                break;
                            } else if let (TextNum::Text(first), TextNum::Text(second)) =
                                // Checks if they are both text
                                (&compare1[item], &compare2[item])
                            {
                                let first_chars: Vec<char> = first.chars().collect(); // Converts the current vector index into its own vector
                                let second_chars: Vec<char> = second.chars().collect();
                                for char in 0..if first.len() <= second.len() {
                                    // Iterates through the shorter vector
                                    if first.len() < second.len() {
                                        bias2 += 1; // Prioritises the longer list appearing after the shorter one
                                    }
                                    first.len()
                                } else {
                                    bias1 += 1;
                                    second.len()
                                } {
                                    match first_chars[char].cmp(&second_chars[char]) {
                                        // Compares the values in alphabetical order
                                        Ordering::Greater => {
                                            bias1 += 1; // Prioritises the later characters in the alphabet appearing after the earlier ones
                                        }
                                        Ordering::Equal => {}
                                        Ordering::Less => {
                                            bias2 += 1;
                                        }
                                    }
                                }
                            } else if let (TextNum::Number(first), TextNum::Number(second)) =
                                // If both are numbers
                                (&compare1[item], &compare2[item])
                            {
                                match first.cmp(&second) {
                                    // Compare the numbers
                                    Ordering::Greater => {
                                        bias1 += 1; // Prioritise the greater number appearing last
                                    }
                                    Ordering::Equal => {}
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
                Ok(File::Names(names)) // Return the list of names
            }
            Err(_) => Err(Error::ReadError), // Return an error if an error is encountered
        }
    }

    fn truncate(name: &mut String, stop_char: &str, pass: u32) -> String {
        // Truncates strings to the designated stop character
        let copy = name.clone();
        let mut length = name.len() - 1;
        let mut found = 0;
        loop {
            if name.ends_with(stop_char) {
                // Checks if the last character is the same as the stop character
                name.remove(length); // Remove it
                length -= 1;
                if found == pass {
                    // Checks if it's passed enough stop characters
                    break;
                }
                found += 1;
            } else {
                if length == 1 {
                    // Returns the original input if no or not enough stop characters found
                    return copy;
                }
                name.remove(length);
                length -= 1;
            }
        }

        name.to_string() // Returns the truncated string
    }

    fn rename(old: &String, name: String) -> Option<Error> {
        // Renames the inputted file or returns an error
        let path = match File::get_directory() {
            // Gets current path
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        match rename(
            // Attempts to rename the file
            format!("{}/{}.wav", path, old),
            format!("{}/{}.wav", path, name),
        ) {
            Ok(_) => (),
            Err(_) => {
                return Some(Error::RenameError); // Return an error if unsuccessful
            }
        };

        match rename(
            format!("{}/{}.bin", path, old),
            format!("{}/{}.bin", path, name),
        ) {
            Ok(_) => (),
            Err(_) => {
                return Some(Error::RenameError);
            }
        };

        None // Return nothing if no error
    }

    fn delete(name: String) -> Option<Error> {
        // Attempts to delete the inputted file or returns an error
        let path = match File::get_directory() {
            Ok(value) => value,
            Err(error) => return Some(error),
        };
        match remove_file(format!("{}/{}.wav", path, name)) {
            Ok(_) => (),
            Err(_) => {
                return Some(Error::DeleteError);
            }
        };
        match remove_file(format!("{}/{}.bin", path, name)) {
            Ok(_) => None,
            Err(_) => None,
        }
    }

    fn exists(new: String, old_list: &Vec<Recording>) -> bool {
        // Checks if a name already exists in the current save
        let mut check = false;
        for item in 0..old_list.len() {
            // Loops through the name sin a list
            if new == old_list[item].name {
                // If it exists return true
                check = true;
                break;
            }
        }

        check
    }

    fn get_directory() -> Result<String, Error> {
        // Gets the working directory
        let mut error = None;
        let mut string = String::new();
        match env::current_exe() {
            // Gets the path that the executable is saved at
            Ok(value) => {
                let mut name = match value.into_os_string().into_string() {
                    // Converts the value into something easier to work with
                    Ok(value) => value,
                    Err(_) => {
                        error = Some(Error::DirectoryError); // Returns an error if unsuccessful
                        string
                    }
                };
                string = File::truncate(&mut name, "/", 2); // Truncates 2 file paths to get the working root
            }
            Err(_) => {
                error = Some(Error::DirectoryError);
            }
        };

        match error {
            // If an error occured at some point in the process, return an error otherwise the file path
            Some(value) => Err(value),
            None => Ok(string),
        }
    }
}

// Types of data that the app works with
enum DataType {
    Settings(Settings),
    SnapShot(SnapShot),
}

// Types of data that the app can load
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

// Recorded input data
#[derive(Savefile, Clone, PartialEq)]
struct SnapShot {
    frames: Vec<([i32; 6], i32)>, // Dial values, frame
}

impl SnapShot {
    fn create(name: &str) -> Option<Error> {
        // Saves an empty snapshot to disk or returns an error
        match SnapShot::new().save(name) {
            Some(error) => {
                return Some(error);
            }
            None => {}
        };

        None
    }

    fn new() -> SnapShot {
        // New snapshot in memory
        SnapShot {
            frames: vec![([0, 0, 0, 0, 0, 0], 0)],
        }
    }

    fn edited(previous: [i32; 6], next: [i32; 6]) -> bool {
        // Checks if the dial values have changed
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
        // Saves a snapshot to disk that doesn't have to be empty - Used when a snapshot already exists
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
        // Creates a preset from dial values
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
        // Sends preset names to UI
        let mut preset_names = vec![];
        for preset in 0..*length {
            preset_names.push(list[preset].name.to_shared_string());
        }

        // ModelRc is the type of list that the UI uses
        ModelRc::new(VecModel::from(preset_names)) // Creates new ModelRc from the names list
    }

    fn send_values(list: &Vec<Preset>, length: &usize) -> ModelRc<ModelRc<i32>> {
        // Sends preset dial values to the UI
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
        // Creates a new recording
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
        // Creates a new recording from a name and dial values
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
        // Parses recording data into dial values
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
        // Parses recording data into a vector
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
        // Parses a vector from dial values
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
        // Sends recording names to UI
        let mut new_list = vec![];

        for recording in 0..list.len() {
            new_list.push(list[recording].name.to_shared_string());
        }

        ModelRc::new(VecModel::from(new_list))
    }

    fn send_values(list: &Vec<Recording>, length: &usize) -> ModelRc<ModelRc<i32>> {
        // Sends recording dial values to UI
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

    fn rename(
        // Renames recordings
        old: &Vec<Recording>,
        new: ModelRc<SharedString>,
    ) -> Result<Vec<Recording>, (Vec<Recording>, Error)> {
        // Returns either a vector of the new names or if there was an error, a vector of new and old names plus an error value
        let mut recording_list = vec![];

        // Checks for different kinds of errors
        let mut fallback_error_occured = false;
        let mut empty_error_occured = false;
        let mut exists_error_occured = false;
        let mut save_file_rename_error_occured = false;
        let mut rename_failed = (false, None); // Occured, Error type

        for name in 0..old.len() {
            // Loops through all the old names
            if new.row_data(name).unwrap() != old[name].name {
                // Checks if the new name doesn't equal the old name
                if new
                    .row_data(name)
                    .unwrap()
                    .contains(&String::from("Default taken..."))
                // Checks if the new name contains the fallback name
                {
                    recording_list.push(Recording::from(&old[name].name, old[name].parse())); // Pushes the old name to the list of names
                    fallback_error_occured = true;
                    break;
                } else if new.row_data(name).unwrap() == String::from("settings") {
                    // Checks if the new name is 'settings'
                    recording_list.push(Recording::from(&old[name].name, old[name].parse()));
                    save_file_rename_error_occured = true;
                    break;
                } else if new.row_data(name).unwrap().is_empty()
                    || new.row_data(name).unwrap() == String::from("")
                // Checks if the new name doesn't exist or equals ''
                {
                    recording_list.push(Recording::from(&old[name].name, old[name].parse()));
                    empty_error_occured = true;
                    break;
                } else if File::exists(String::from(new.row_data(name).unwrap()), &old) {
                    // Checks if the new name already exists
                    recording_list.push(Recording::from(&old[name].name, old[name].parse()));
                    exists_error_occured = true;
                    break;
                } else {
                    match File::rename(&old[name].name, String::from(new.row_data(name).unwrap())) {
                        // Renames file if all the checks pass
                        Some(error) => {
                            rename_failed = (true, Some(error));
                        }
                        None => {}
                    }
                    recording_list.push(Recording::from(
                        &String::from(new.row_data(name).unwrap()),
                        old[name].parse(),
                    )); // Pushes new name to list
                }
            } else {
                recording_list.push(Recording::from(&old[name].name, old[name].parse()));
                // Skips recordings that were unchanged
            }
        }

        if exists_error_occured {
            // Checks if any errors occured and returns them and a list or just a list
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
        // Shuffles recordings
        let mut new = vec![];
        let mut avaliable = vec![];

        for number in 0..length {
            // Creates a list of numbers 0 to list length -1
            avaliable.push(number);
        }

        for _ in 0..length {
            let random = random_range(0..avaliable.len()); // Creates a random number between 0 and the length of the avaliable numbers list
            new.push(avaliable[random] as i32); // Pushes the value at the index to the shuffle list
            avaliable.remove(random); // Removes the used number from the avaliable list
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
        // Creates empty settings data
        Settings {
            presets: vec![],
            recordings: vec![],
        }
    }

    fn get_index_data(&self) -> IndexData {
        // Gets the length of each list in the settings struct
        IndexData {
            preset_length: self.presets.len(),
            recording_length: self.recordings.len(),
        }
    }

    fn sync(&mut self, ui: &AppWindow) {
        // Sync settings data with files and UI
        let index_data = self.get_index_data();

        let mut dials = [0, 0, 0, 0, 0, 0];
        if self.recordings.len() > 0 {
            for index in 0..6 {
                match ui.get_current_dial_values().row_data(index) {
                    Some(value) => dials[index] = value,
                    None => {
                        dials = [0, 0, 0, 0, 0, 0];
                        break;
                    }
                };
                // Gets dial values from UI
            }
        }

        // Check for new preset creation
        if ui.get_new_preset_created() {
            self.presets.push(Preset::from(dials)); // Update the settings data with the new preset created from the values of the dials
        }

        // Check for preset deletion
        if ui.get_preset_deleted() {
            if self.presets.len() > ui.get_deleted_preset_index() as usize {
                self.presets.remove(ui.get_deleted_preset_index() as usize); // Deletes deleted preset from settings data
                ui.set_can_delete(true); // Tells the UI that the item has finished being deleted to enable more things to be deleted
            }
        }

        // Check for preset rename
        if ui.get_preset_renamed() {
            for preset in 0..index_data.preset_length {
                self.presets[preset].name =
                    String::from(match ui.get_preset_names().row_data(preset) {
                        // Renames preset with the value in the UI
                        Some(name) => name,
                        None => SharedString::from("New Preset"), // Sets to default value if something went wrong retrieving the new name form the UI
                    });
            }
        }

        // Check for recording edits
        if index_data.recording_length > 0 {
            let position = ui.get_current_recording() as usize;
            if ui.get_dials_edited() {
                self.recordings[position] = Recording::from(&self.recordings[position].name, dials);
                // Updates settings data with edited values
            }
        }

        // Check for recording deletion
        if ui.get_recording_deleted() {
            self.recordings
                .remove(ui.get_deleted_recording_index() as usize); // Removes recording data from settings
            ui.set_can_delete(true);
        }

        // Check for recording renaming
        if ui.get_recording_renamed() {
            self.recordings = match Recording::rename(&self.recordings, ui.get_recording_names()) {
                // Renames recording
                Ok(value) => value,
                Err(error) => {
                    error.1.send(ui); // Sends error value to UI
                    error.0
                }
            };
        }

        // Sync recording data with any changes that might have been made to the application files
        let path = match File::get_directory() {
            Ok(value) => value,
            Err(error) => {
                error.send(ui);
                String::new()
            }
        };
        let file_names = match File::search(&path, "wav", true) {
            // Gets wav file names
            Ok(File::Names(value)) => value,
            Err(error) => {
                error.send(ui);
                vec![String::from("Couldn't read files")]
            }
        };

        let mut snapshot_names = match File::search(&path, "bin", true) {
            // Gets binary file names
            Ok(File::Names(value)) => value,
            Err(error) => {
                error.send(ui);
                vec![String::from("Couldn't read files")]
            }
        };

        for name in 0..snapshot_names.len() {
            if snapshot_names[name] == "settings" {
                snapshot_names.remove(name); // Removes the settings file from the list of binary files
                break;
            }
        }

        let mut updated_recordings = vec![];

        if file_names.len() > 0 {
            for name in 0..file_names.len() {
                // Loops over all the names
                if self.recordings.len() > 0 {
                    for recording in 0..self.recordings.len() {
                        if self.recordings[recording].name == file_names[name] {
                            // If the recording is known, then add the old recording to the list
                            updated_recordings.push(Recording::from(
                                &file_names[name],
                                Recording::parse(&self.recordings[recording]),
                            ));
                            break;
                        }
                        if recording == self.recordings.len() - 1 {
                            updated_recordings.push(Recording::new(&file_names[name]));
                            // If it's unknown then create a new recording
                        }
                    }
                } else {
                    updated_recordings.push(Recording::new(&file_names[name])); // Adds new recording to settings data
                }

                // Syncs snapshots
                if snapshot_names.len() > 0 {
                    for file in 0..snapshot_names.len() {
                        if snapshot_names.len() > 0 {
                            if file_names[name] != snapshot_names[file] {
                                // If the names of the files and snapshots don't match then create a new snapshot file
                                match SnapShot::create(&file_names[name]) {
                                    Some(error) => {
                                        error.send(ui);
                                    }
                                    None => (),
                                }
                            } else {
                                snapshot_names.remove(file); // Remove snapshot name from list so that the next check doesn't autoatically fail
                                break;
                            }
                        }
                    }
                } else {
                    match SnapShot::create(&file_names[name]) {
                        // Creates a new snapshot if there's a file but no snapshots
                        Some(error) => {
                            error.send(ui);
                        }
                        None => (),
                    }
                }
            }
        }

        self.recordings = updated_recordings; // Updates the settings data with the updated data
    }
}

// Keeps track of the settings, the recording thread, whether recordings are being played, and the values of the dials during a set of audio frames
struct Tracker {
    settings: Arc<RwLock<Settings>>,
    locked: Arc<RwLock<Recording>>, // Values to hold while locked
    playing: Arc<RwLock<bool>>,     // Something is playing
    snapshot_frame_values: Arc<RwLock<[i32; 6]>>, // Values of the currently active snapshot frame group
    empty_recording: Arc<RwLock<bool>>,           // Whether the newest reecording is empty
    recording_check: Arc<RwLock<bool>>, // Whether a recording is in progress or just happened
    preloaded: Arc<RwLock<bool>>,       // Whether any audio data is loaded in memory
}

impl Tracker {
    fn new(settings: Settings) -> Tracker {
        // Creates a new tracker
        Tracker {
            settings: Arc::new(RwLock::new(settings)),
            locked: Arc::new(RwLock::new(Recording::new(&String::new()))),
            playing: Arc::new(RwLock::new(false)),
            snapshot_frame_values: Arc::new(RwLock::new([0, 0, 0, 0, 0, 0])),
            empty_recording: Arc::new(RwLock::new(true)),
            recording_check: Arc::new(RwLock::new(false)),
            preloaded: Arc::new(RwLock::new(false)),
        }
    }

    fn write<T>(handle: Arc<RwLock<T>>, set: T) {
        // Wrtes data to tracked data
        let mut writer = handle.write().unwrap();
        *writer = set;
    }

    fn read<T: Copy>(handle: Arc<RwLock<T>>) -> T {
        // Reads and returns tracked data
        let reader = handle.read().unwrap();
        *reader
    }
}

// -------- Functions --------
fn save(data: DataType, file: &str) -> Option<Error> {
    // Save data to files
    let path = match File::get_directory() {
        Ok(value) => value,
        Err(error) => return Some(error),
    };
    match data {
        // Checks if saving settings data or snapshot data
        DataType::Settings(value) => match save_file(format!("{}/{}.bin", path, file), 0, &value) {
            // Saves settings daat
            Ok(_) => {
                return None;
            }
            Err(_) => {
                return Some(Error::SaveError);
            }
        },
        DataType::SnapShot(value) => match save_file(format!("{}/{}.bin", path, file), 0, &value) {
            // Saves snapshot data
            Ok(_) => {
                return None;
            }
            Err(_) => match save_file(format!("{}.bin", file), 0, &value) {
                // Tries again but without the path variable incase file was inputted as a path
                Ok(_) => None,
                Err(_) => Some(Error::SaveError),
            },
        },
    }
}

fn load(file: &str, kind: LoadType) -> Result<DataType, Error> {
    // Loads data from file
    let path = match File::get_directory() {
        Ok(value) => value,
        Err(error) => return Err(error),
    };
    match kind {
        // Checks to see what kind of data it should be loading
        LoadType::Settings => match load_file(format!("{}/{}.bin", path, file), 0) {
            // Loads settings data
            Ok(value) => {
                return Ok(DataType::Settings(value));
            }
            Err(_) => {
                return Err(Error::LoadError);
            }
        },
        LoadType::Snapshot => match load_file(format!("{}/{}.bin", path, file), 0) {
            // Loads snapshot data
            Ok(value) => {
                return Ok(DataType::SnapShot(value));
            }
            Err(_) => {
                return Err(Error::LoadError);
            }
        },
    }
}

fn main() -> Result<(), Box<dyn STDError>> {
    let ui = AppWindow::new()?;

    let errors = Arc::new(RwLock::new(None)); // Creates error handler

    // Creates a variable that can be used across threads and move blocks and can be read from without locking
    let tracker = Arc::new(Tracker::new(match load("settings", LoadType::Settings) {
        Ok(DataType::Settings(value)) => value, // Loads settings
        Ok(DataType::SnapShot(_)) => {
            // If passed snapshot data then create new settings and save the file
            Tracker::write(errors.clone(), Some(Error::LoadError));
            match save(DataType::Settings(Settings::new()), "settings") {
                Some(error) => {
                    Tracker::write(errors.clone(), Some(error));
                }
                None => {}
            };
            Settings::new()
        }
        Err(_) => {
            match save(DataType::Settings(Settings::new()), "settings") {
                Some(error) => {
                    Tracker::write(errors.clone(), Some(error));
                }
                None => {}
            };
            Settings::new() // Creates new settings if it didn't exist already
        }
    }));

    let (record_sender, record_receiver) = mpsc::channel::<Message>(); // Creates recorder message sender and receiver

    // Creates references to the required values in the tracker
    let record_error_handle = errors.clone();
    let recording_empty_handle = tracker.empty_recording.clone();
    let check = tracker.recording_check.clone();
    match thread::Builder::new() // Spawns a new thread for recording audio
        .name(String::from("Recorder"))
        .spawn(move || {
            let audio_spec = WavSpec {
                // Decides on the settings of the recording
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
                }
            };

            let empty = recording_empty_handle.clone(); // New reference for the loop do avoid memory issues
            loop {
                match record_receiver.recv() {
                    // Blocks until message received
                    Ok(Message::StartRecording) => (),
                    _ => {
                        Tracker::write(record_error_handle.clone(), Some(Error::MessageError));
                        continue; // Write an error and start looking for another message
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
                    // Checks how many times something has had to been renamed to the fallback name
                    if (*name).contains(&String::from("Default taken...")) {
                        fallbacks += 1;
                    }
                }

                let recording_amount = taken_names.len();

                let mut new_name = String::new();

                if recording_amount > 0 {
                    let potential = format!("Recording {}", recording_amount + 1); // Tests a potential name
                    for item in 0..recording_amount {
                        if potential != taken_names[item] {
                            // If the potential name isn't already a thing
                            new_name = format!("{}.wav", potential); // Update new name
                        } else {
                            new_name = format!("Default taken... {}.wav", fallbacks + 1); // Makes a new default taken name if it has been taken
                            break;
                        }
                    }
                } else {
                    new_name = String::from("Recording 1.wav"); // Creates this name if first recording
                }

                let mut writer = // Creates a new writer
                    match WavWriter::create(format!("{}/{}", path, new_name), audio_spec) {
                        Ok(value) => value,
                        Err(_) => {
                            Tracker::write(record_error_handle.clone(), Some(Error::WriteError));
                            continue;
                        }
                    };

                let mut initial_silence = true;

                let empty2 = empty.clone(); // New reference to avoid more memory issues
                let record_callback = move |data: RUBuffers| {
                    // Run when callback called
                    let mut interleaved = vec![];

                    let channel1_len = data[0].len();
                    let channel2_len = data[1].len();

                    for sample in 0..(if channel1_len > channel2_len {
                        // Loops through the channel with the least amount of data
                        channel2_len
                    } else {
                        channel1_len
                    }) {
                        if initial_silence {
                            if data[0][sample] != 0.0 || data[1][sample] != 0.0 {
                                // If either channel has audio playing
                                initial_silence = false;
                                Tracker::write(empty2.clone(), false); // Tells the tracker that this recording should be saved
                                continue;
                            } else {
                                continue;
                            }
                        } else {
                            // Pushes the data from each channel to the interleaved list
                            interleaved.push(data[0][sample]);
                            interleaved.push(data[1][sample]);
                        }
                    }

                    if !initial_silence {
                        for sample in &interleaved {
                            writer.write_sample(*sample).unwrap(); // Writes the data from the interleaved list to file
                        }
                    }
                };

                let callback = rucallback!(record_callback); // Initiates a callback

                let mut recorder = RUHear::new(callback); // Creates a new recorder

                match recorder.start() {
                    // Starts a recorder
                    Ok(_) => {}
                    Err(_) => {
                        Tracker::write(record_error_handle.clone(), Some(Error::RecordError));
                        continue;
                    }
                };

                loop {
                    match record_receiver.recv() {
                        // Blocks until a stop message is received
                        Ok(Message::StopRecording) => break,
                        _ => {
                            Tracker::write(record_error_handle.clone(), Some(Error::MessageError));
                            continue;
                        }
                    }
                }

                match recorder.stop() {
                    // Stops recording
                    Ok(_) => {}
                    Err(_) => {
                        Tracker::write(record_error_handle.clone(), Some(Error::RecordError));
                        continue;
                    }
                };

                if Tracker::read(empty.clone()) {
                    // If recording empty
                    match File::delete(File::truncate(&mut new_name, ".", 0)) {
                        // Delete any recording data that had been saved so far
                        Some(_) => {
                            Tracker::write(
                                record_error_handle.clone(),
                                Some(Error::EmptyRecordingError),
                            );
                        }
                        None => (),
                    }
                }
            }
        }) {
        Ok(_) => (),
        Err(_) => {
            Tracker::write(errors.clone(), Some(Error::RecorderThreadError)); // Error if thread fails to start
        }
    };

    let (audio_sender, audio_receiver) = mpsc::channel::<Message>(); // Message sender and reciever for audio playback

    // Creates references for required values in audio thread
    let player_error_handle = errors.clone();
    let player_settings_handle = tracker.settings.clone();
    let player_frame_handle = tracker.snapshot_frame_values.clone();
    let player_finished = tracker.playing.clone();
    let loaded = tracker.preloaded.clone();
    match thread::Builder::new() // Creates audio thread
        .name(String::from("Player"))
        .spawn(move || {
            // Initialises some variables
            let mut sound_data;

            let mut length;

            let mut file;

            'one: loop {
                match audio_receiver.recv() {
                    // Blocks until a load file message is received
                    Ok(Message::File(name)) => {
                        file = name;
                        sound_data = match StaticSoundData::from_file(&file) {
                            // Loads audio data from file
                            Ok(value) => {
                                length = value.duration(); // Gets the length of the audio
                                Tracker::write(loaded.clone(), true);
                                value
                            }
                            Err(_) => {
                                Tracker::write(player_error_handle.clone(), Some(Error::ReadError));
                                continue 'one;
                            }
                        };
                    }
                    _ => {
                        Tracker::write(player_error_handle.clone(), Some(Error::MessageError));
                        continue 'one;
                    }
                }

                'two: loop {
                    let mut capturing = false;
                    match audio_receiver.recv() {
                        // Blocks until message received
                        Ok(Message::File(_)) => break 'two, // Breaks the second loop to load a file
                        Ok(Message::PlayAudio(mut playback)) => {
                            if let Playback::Capture(_) = playback.0 {
                                capturing = true; // Sets capturing check to true if playback type is Capture
                            }
                            let mut audio_manager = match AudioManager::<DefaultBackend>::new(
                                // Create a new audio manager
                                AudioManagerSettings::default(),
                            ) {
                                Ok(value) => value,
                                Err(_) => {
                                    Tracker::write(
                                        player_error_handle.clone(),
                                        Some(Error::PlaybackError),
                                    );
                                    continue 'two;
                                }
                            };

                            // Filter setup
                            let sub_bass =
                                EqFilterBuilder::new(EqFilterKind::LowShelf, 40.0, 0.0, 1.0);
                            let bass = EqFilterBuilder::new(EqFilterKind::Bell, 155.0, 0.0, 0.82);
                            let low_mids =
                                EqFilterBuilder::new(EqFilterKind::Bell, 625.0, 0.0, 0.83);
                            let high_mids =
                                EqFilterBuilder::new(EqFilterKind::Bell, 1500.0, 0.0, 1.5);
                            let treble =
                                EqFilterBuilder::new(EqFilterKind::HighShelf, 12000.0, 0.0, 0.75);
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
                                // Creates a track with the filter handles enabled
                                Ok(value) => value,
                                Err(_) => {
                                    Tracker::write(
                                        player_error_handle.clone(),
                                        Some(Error::PlaybackError),
                                    );
                                    continue 'two;
                                }
                            };

                            let _ = match track.play(sound_data.clone()) {
                                // Plays the track
                                Ok(value) => value,
                                Err(_) => {
                                    Tracker::write(
                                        player_error_handle.clone(),
                                        Some(Error::PlaybackError),
                                    );
                                    continue 'two;
                                }
                            };

                            let start = Instant::now(); // Gets the time the track started playing
                            let mut frame: usize = 0;
                            let mut previous_frame = [0, 0, 0, 0, 0, 0];
                            let mut edited_frame: usize = 0;
                            let mut snapshot = if let Playback::Capture(ref data) = playback.0 {
                                // Gets snapshot data
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
                                // Loops while the time spent playing is less than the length of the audio
                                match audio_receiver.try_recv() {
                                    // Blocks until a file, stop, or playback message is received
                                    Ok(Message::StopAudio) => {
                                        if capturing {
                                            snapshot.frames.remove(0);
                                            match snapshot.save(&File::truncate(&mut file.clone(), ".", 0)) // Saves new snapshot data to file if capturing
                                            {
                                                Some(error) => {
                                                    Tracker::write(
                                                        player_error_handle.clone(),
                                                        Some(error),
                                                    );
                                                }
                                                None => (),
                                            };
                                        }
                                        continue 'two; // Stops audio
                                    }
                                    Ok(Message::File(_)) => {
                                        if capturing {
                                            snapshot.frames.remove(0);
                                            match snapshot.save(&File::truncate(
                                                &mut file.clone(),
                                                ".",
                                                0,
                                            )) {
                                                Some(error) => {
                                                    Tracker::write(
                                                        player_error_handle.clone(),
                                                        Some(error),
                                                    );
                                                }
                                                None => (),
                                            };
                                        }
                                        break 'two; // Loads new audio data
                                    }
                                    Ok(Message::PlayAudio((Playback::Capture(_), _))) => {
                                        if capturing {
                                            snapshot.frames.remove(0);
                                            match snapshot.save(&File::truncate(
                                                &mut file.clone(),
                                                ".",
                                                0,
                                            )) {
                                                Some(error) => {
                                                    Tracker::write(
                                                        player_error_handle.clone(),
                                                        Some(error),
                                                    );
                                                }
                                                None => (),
                                            };
                                        }
                                        continue 'two; // Stops playing
                                    }
                                    Ok(Message::PlayAudio((value, _))) => {
                                        // Changes type of playback
                                        playback.0 = value;
                                        if let Playback::Input(ref frames) = playback.0 {
                                            snapshot = frames.clone();
                                            Tracker::write(
                                                player_frame_handle.clone(),
                                                snapshot.frames[edited_frame].0,
                                            );
                                        }
                                    }
                                    _ => (),
                                }
                                if let Playback::Input(_) = playback.0 {
                                    // If playback type equals input playback
                                    if edited_frame < snapshot.frames.len() {
                                        if frame == snapshot.frames[edited_frame].1 as usize {
                                            // If current frame is the same as the one saved in the the snapshot data
                                            Tracker::write(
                                                player_frame_handle.clone(),
                                                snapshot.frames[edited_frame].0,
                                            ); // Write dial data
                                               // Set the handle values to edit the audio based on snapshot data
                                            sub_bass_handle.set_gain(
                                                if snapshot.frames[edited_frame].0[0] == -7 {
                                                    -60.0 // Make silent if value is -7
                                                } else {
                                                    snapshot.frames[edited_frame].0[0] as f32 * 4.0
                                                    // Multiply dial value by 4 to hear a difference
                                                },
                                                Tween::default(),
                                            );
                                            bass_handle.set_gain(
                                                if snapshot.frames[edited_frame].0[1] == -7 {
                                                    -60.0
                                                } else {
                                                    snapshot.frames[edited_frame].0[1] as f32 * 4.0
                                                },
                                                Tween::default(),
                                            );
                                            low_mids_handle.set_gain(
                                                if snapshot.frames[edited_frame].0[2] == -7 {
                                                    -60.0
                                                } else {
                                                    snapshot.frames[edited_frame].0[2] as f32 * 4.0
                                                },
                                                Tween::default(),
                                            );
                                            high_mids_handle.set_gain(
                                                if snapshot.frames[edited_frame].0[3] == -7 {
                                                    -60.0
                                                } else {
                                                    snapshot.frames[edited_frame].0[3] as f32 * 4.0
                                                },
                                                Tween::default(),
                                            );
                                            treble_handle.set_gain(
                                                if snapshot.frames[edited_frame].0[4] == -7 {
                                                    -60.0
                                                } else {
                                                    snapshot.frames[edited_frame].0[4] as f32 * 4.0
                                                },
                                                Tween::default(),
                                            );
                                            panning_handle.set_panning(
                                                snapshot.frames[edited_frame].0[5] as f32 * 0.15, // Multiply panning by 0.15 as panning is more sensitive to changes
                                                Tween::default(),
                                            );
                                        }
                                    }
                                } else {
                                    let settings = player_settings_handle.read().unwrap();

                                    if let Playback::Capture(_) = playback.0 {
                                        // If capturing inputs
                                        if SnapShot::edited(
                                            // Checks if a change has been made to the dials since the last change
                                            previous_frame,
                                            Recording::parse(&settings.recordings[playback.1]),
                                        ) {
                                            snapshot.frames.push((
                                                // Pushes new values to list
                                                Recording::parse(&settings.recordings[playback.1]),
                                                frame as i32,
                                            ));
                                            previous_frame = snapshot.frames[edited_frame].0; // Updates the previous frame for next check
                                            edited_frame += 1;
                                        }
                                    }

                                    // Set the handle values based on settings
                                    sub_bass_handle.set_gain(
                                        if settings.recordings[playback.1].sub_bass == -7 {
                                            -60.0
                                        } else {
                                            settings.recordings[playback.1].sub_bass as f32 * 4.0
                                        },
                                        Tween::default(),
                                    );
                                    bass_handle.set_gain(
                                        if settings.recordings[playback.1].bass == -7 {
                                            -60.0
                                        } else {
                                            settings.recordings[playback.1].bass as f32 * 4.0
                                        },
                                        Tween::default(),
                                    );
                                    low_mids_handle.set_gain(
                                        if settings.recordings[playback.1].low_mids == -7 {
                                            -60.0
                                        } else {
                                            settings.recordings[playback.1].low_mids as f32 * 4.0
                                        },
                                        Tween::default(),
                                    );
                                    high_mids_handle.set_gain(
                                        if settings.recordings[playback.1].high_mids == -7 {
                                            -60.0
                                        } else {
                                            settings.recordings[playback.1].high_mids as f32 * 4.0
                                        },
                                        Tween::default(),
                                    );
                                    treble_handle.set_gain(
                                        if settings.recordings[playback.1].treble == -7 {
                                            -60.0
                                        } else {
                                            settings.recordings[playback.1].treble as f32 * 4.0
                                        },
                                        Tween::default(),
                                    );
                                    panning_handle.set_panning(
                                        settings.recordings[playback.1].pan as f32 * 0.15,
                                        Tween::default(),
                                    );

                                    drop(settings); // Drop read access of settings
                                }

                                if !capturing {
                                    // Increases edited frame if equal to snapshot data so it remains in sync if you swap playback type
                                    if frame
                                        == snapshot.frames[if edited_frame < snapshot.frames.len() {
                                            edited_frame
                                        } else {
                                            edited_frame - 1
                                        }]
                                        .1 as usize
                                    {
                                        edited_frame += 1;
                                    }
                                }
                                frame += 1;

                                thread::sleep(Duration::from_millis(20)); // Sleeps thread for 20 milliseconds
                            }

                            Tracker::write(player_finished.clone(), true); // Tells the tracker that playback is finished

                            if capturing {
                                // Saves captured inputs to file
                                match snapshot.save(&File::truncate(&mut file.clone(), ".", 0)) {
                                    Some(error) => {
                                        Tracker::write(player_error_handle.clone(), Some(error));
                                    }
                                    None => (),
                                };
                            }
                        }
                        Ok(Message::StopAudio) => continue 'two, // Waits to play again
                        _ => {
                            Tracker::write(player_error_handle.clone(), Some(Error::MessageError)); // Writes error if incorrect message sent to thread
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

    // Update callback
    ui.on_update({
        let ui_handle = ui.as_weak();

        let startup_ref_count = tracker.settings.clone();

        let error_handle = errors.clone();

        move || {
            let ui = ui_handle.unwrap();

            match Tracker::read(error_handle.clone()) {
                // Checks for errors
                Some(error) => {
                    error.send(&ui);
                    Tracker::write(error_handle.clone(), None);
                }
                None => {}
            };

            if ui.get_started() {
                // Syncs settings data on initial load
                // Acquires write access to the loaded data
                let mut settings = startup_ref_count.write().unwrap();
                settings.sync(&ui);
            }

            // Aquires read access to the loaded data
            let settings = startup_ref_count.read().unwrap();

            let index_data = settings.get_index_data();

            // Sends a list of preset names to the ui to be displayed
            ui.set_preset_names(Preset::send_names(
                &settings.presets,
                &index_data.preset_length,
            ));

            // Sends a nested list of preset values to the ui to be displayed
            ui.set_preset_values(Preset::send_values(
                &settings.presets,
                &index_data.preset_length,
            ));

            // Sends recording names to the ui to be displayed
            ui.set_recording_names(Recording::send_names(&settings.recordings));

            // Sends recording values to the ui to be displayed
            if !ui.get_locked() {
                ui.set_recording_values(Recording::send_values(
                    &settings.recordings,
                    &index_data.recording_length,
                ));
            }

            if ui.get_current_recording() < settings.recordings.len() as i32 {
                // Sets dial values to current recording data
                ui.set_current_dial_values(ModelRc::new(VecModel::from(
                    settings.recordings[ui.get_current_recording() as usize]
                        .parse_vec_from_recording(),
                )));
            }
        }
    });

    // Updates locked values
    ui.on_update_locked_values({
        let ui_handle = ui.as_weak();

        let settings_handle = tracker.settings.clone();

        let locked_handle = tracker.locked.clone();

        move || {
            let ui = ui_handle.unwrap();

            let settings = settings_handle.read().unwrap();

            let mut locked = locked_handle.write().unwrap();

            if settings.recordings.len() > 0 {
                // Sets locked vales to current recording data
                ui.set_dial_values_when_locked(Recording::send_values(
                    &settings.recordings,
                    &settings.get_index_data().recording_length,
                ));
                // Sets tracker locked values
                *locked = settings.recordings[ui.get_current_recording() as usize].clone();
            }
        }
    });

    // Syncs UI and settings with current locked values
    ui.on_sync_with_locked_values({
        let ui_handle = ui.as_weak();

        let settings_handle = tracker.settings.clone();

        let locked_handle = tracker.locked.clone();

        move || {
            let ui = ui_handle.unwrap();

            let mut settings = settings_handle.write().unwrap();

            let locked = locked_handle.read().unwrap();

            // Sets settings data to locked values
            settings.recordings[ui.get_current_recording() as usize].sub_bass = locked.sub_bass;
            settings.recordings[ui.get_current_recording() as usize].bass = locked.bass;
            settings.recordings[ui.get_current_recording() as usize].low_mids = locked.low_mids;
            settings.recordings[ui.get_current_recording() as usize].high_mids = locked.high_mids;
            settings.recordings[ui.get_current_recording() as usize].treble = locked.treble;
            settings.recordings[ui.get_current_recording() as usize].pan = locked.pan;

            // Sets dials to locked values
            ui.set_current_dial_values(ModelRc::new(VecModel::from(
                settings.recordings[ui.get_current_recording() as usize].parse_vec_from_recording(),
            )));
        }
    });

    // Saves settings to file and memory
    ui.on_save({
        let ui_handle = ui.as_weak();

        let update_ref_count = tracker.settings.clone();

        let empty = tracker.empty_recording.clone();

        let just_recorded = tracker.recording_check.clone();

        move || {
            let ui = ui_handle.unwrap();

            // Skips if an empty recording was just created
            if Tracker::read(empty.clone()) && Tracker::read(just_recorded.clone()) {
                Tracker::write(just_recorded.clone(), false);
                return;
            }

            // This block is used to drop the write lock on the stored data as soon as the last write is completed
            // This frees it to be used in the function called underneath and in any threads where it is needed
            {
                // Acquires write access to the loaded data
                let mut settings = update_ref_count.write().unwrap();
                settings.sync(&ui); // Syncs settings data
            }

            ui.invoke_update(); // Updates UI

            // Aquires read access to the loaded data
            let settings = update_ref_count.read().unwrap();
            // Save data if not locked or recording inputs
            if !ui.get_locked() && !ui.get_input_recording() {
                match save(DataType::Settings((*settings).clone()), "settings") {
                    Some(error) => {
                        error.send(&ui);
                    }
                    None => {}
                }
            }
        }
    });

    // Starts and stops recordings
    ui.on_record({
        let ui_handle = ui.as_weak();

        let sender_handle = record_sender.clone();

        let error_handle = errors.clone();

        move || {
            let ui = ui_handle.unwrap();

            match sender_handle.send(if ui.get_recording() {
                // Sends message to recording thread
                // Sends stop message and updates UI
                ui.set_recording(false);
                Message::StopRecording
            } else {
                // Sends start message and updates UI
                ui.set_recording(true);
                Message::StartRecording
            }) {
                Ok(_) => {
                    if !ui.get_recording() {
                        // If UI not recording then save and shuffle songs
                        ui.invoke_save();
                        ui.invoke_gen_shuffle();
                        ui.invoke_skip_audio();
                        ui.invoke_skip_audio();
                    }
                }
                Err(_) => {
                    Tracker::write(error_handle.clone(), Some(Error::MessageError));
                }
            }
        }
    });

    // Deletes recordings
    ui.on_delete_recordings({
        let ui_handle = ui.as_weak();

        move || {
            let ui = ui_handle.unwrap();

            match File::delete(String::from(ui.get_deleted_recording_name())) {
                // Deletes recordings
                Some(error) => {
                    error.send(&ui);
                }
                None => {}
            };

            ui.invoke_save(); // Saves changes
        }
    });

    // Skips song
    ui.on_skip_audio({
        let ui_handle = ui.as_weak();

        let error_handle = errors.clone();

        let sender_handle = audio_sender.clone();

        let settings_handle = tracker.settings.clone();

        let preloaded_handle = tracker.preloaded.clone();

        move || {
            let ui = ui_handle.unwrap();

            let settings = settings_handle.read().unwrap();

            Tracker::write(preloaded_handle.clone(), false); // Tells thread that nothing has been preloaded

            let file = if settings.recordings.len() > 0 {
                // Gets the name of the recording that should be played
                settings.recordings[ui.get_current_recording() as usize]
                    .name
                    .clone()
            } else {
                String::new()
            };

            let path = match File::get_directory() {
                Ok(value) => value,
                Err(error) => {
                    error.send(&ui);
                    String::new()
                }
            };

            let snapshot_data = if settings.recordings.len() > 0 {
                // Loads the snapshot data of that recording
                match load(
                    &settings.recordings[ui.get_current_recording() as usize].name,
                    LoadType::Snapshot,
                ) {
                    Ok(DataType::SnapShot(data)) => data,
                    _ => {
                        Error::LoadError.send(&ui);
                        SnapShot::new()
                    }
                }
            } else {
                SnapShot::new()
            };

            if settings.recordings.len() > 0 {
                for _ in 0..if ui.get_starting_threads() {
                    // If threads are starting for the first time only send load messgae once, otherwise twice
                    ui.set_starting_threads(false);
                    1
                } else {
                    2
                } {
                    match sender_handle.send(Message::File(format!("{}/{}.wav", path, file))) {
                        // Sends load message and file path
                        Ok(_) => (),
                        Err(_) => {
                            Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                        }
                    }
                }
                if ui.get_audio_playback() {
                    // If already generic playing
                    match sender_handle.send(Message::PlayAudio((
                        // Sends message to play new recording as a generic playback along with snapshot data
                        Playback::Generic(snapshot_data),
                        ui.get_current_recording() as usize,
                    ))) {
                        Ok(_) => (),
                        Err(_) => {
                            Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                        }
                    }
                } else if ui.get_input_playback() {
                    // If already input playback
                    match sender_handle.send(Message::PlayAudio((
                        // Sends message to play new recordings input data along with its snapshot data
                        Playback::Input(snapshot_data),
                        ui.get_current_recording() as usize,
                    ))) {
                        Ok(_) => (),
                        Err(_) => {
                            Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                        }
                    }
                } else if ui.get_input_recording() {
                    // If recording inputs
                    for _ in 0..2 {
                        let snapshot_data = SnapShot::new(); // Send message to record inputs twice
                        match sender_handle.send(Message::PlayAudio((
                            Playback::Capture(snapshot_data),
                            ui.get_current_recording() as usize,
                        ))) {
                            Ok(_) => (),
                            Err(_) => {
                                Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                            }
                        }
                    }
                }
            }
        }
    });

    // On generic playback
    ui.on_play_generic({
        let ui_handle = ui.as_weak();

        let error_handle = errors.clone();

        let sender_handle = audio_sender.clone();

        let settings_handle = tracker.settings.clone();

        let preloaded_handle = tracker.preloaded.clone();

        move || {
            let ui = ui_handle.unwrap();

            let settings = settings_handle.read().unwrap();

            let snapshot_data = match load(
                // Load snapshot data
                &settings.recordings[ui.get_current_recording() as usize].name,
                LoadType::Snapshot,
            ) {
                Ok(DataType::SnapShot(data)) => data,
                _ => {
                    Error::LoadError.send(&ui);
                    return;
                }
            };

            if Tracker::read(preloaded_handle.clone()) {
                () // Do nothing if data has been preloaded
            } else {
                // Load new data
                let file = if settings.recordings.len() > 0 {
                    settings.recordings[ui.get_current_recording() as usize]
                        .name
                        .clone()
                } else {
                    String::new()
                };

                let path = match File::get_directory() {
                    Ok(value) => value,
                    Err(error) => {
                        error.send(&ui);
                        String::new()
                    }
                };

                match sender_handle.send(Message::File(format!("{}/{}.wav", path, file))) {
                    Ok(_) => (),
                    Err(_) => {
                        Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                    }
                }
            }

            match sender_handle.send(if ui.get_audio_playback() {
                // Send message to start and stop playback and update UI accordingly
                ui.set_audio_playback(false);
                ui.set_input_playback(false);
                ui.set_input_recording(false);
                Message::StopAudio
            } else {
                ui.set_audio_playback(true);
                ui.set_input_playback(false);
                ui.set_input_recording(false);
                Message::PlayAudio((
                    Playback::Generic(snapshot_data),
                    ui.get_current_recording() as usize,
                ))
            }) {
                Ok(_) => (),
                Err(_) => {
                    Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                }
            }

            ui.set_current_dial_values(ModelRc::new(VecModel::from(
                // Update dial values
                settings.recordings[ui.get_current_recording() as usize].parse_vec_from_recording(),
            )));
        }
    });

    // Input playback
    ui.on_play_captured_inputs({
        let ui_handle = ui.as_weak();

        let settings_handle = tracker.settings.clone();

        let dials = tracker.snapshot_frame_values.clone();

        let error_handle = errors.clone();

        let sender_handle = audio_sender.clone();

        let preloaded_handle = tracker.preloaded.clone();

        move || {
            let ui = ui_handle.unwrap();

            let settings = settings_handle.read().unwrap();

            let snapshot_data = match load(
                // Load snapshot data
                &settings.recordings[ui.get_current_recording() as usize].name,
                LoadType::Snapshot,
            ) {
                Ok(DataType::SnapShot(data)) => data,
                _ => {
                    Error::LoadError.send(&ui);
                    return;
                }
            };

            Tracker::write(
                dials.clone(),
                Recording::parse(&settings.recordings[ui.get_current_recording() as usize]),
            );

            if Tracker::read(preloaded_handle.clone()) {
                ()
            } else {
                let file = if settings.recordings.len() > 0 {
                    settings.recordings[ui.get_current_recording() as usize]
                        .name
                        .clone()
                } else {
                    String::new()
                };

                let path = match File::get_directory() {
                    Ok(value) => value,
                    Err(error) => {
                        error.send(&ui);
                        String::new()
                    }
                };

                match sender_handle.send(Message::File(format!("{}/{}.wav", path, file))) {
                    Ok(_) => (),
                    Err(_) => {
                        Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                    }
                }
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
                Message::PlayAudio((
                    Playback::Input(snapshot_data),
                    ui.get_current_recording() as usize,
                ))
            }) {
                Ok(_) => (),
                Err(_) => {
                    Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                }
            }
        }
    });

    // Record inputs
    ui.on_capture_inputs({
        let ui_handle = ui.as_weak();

        let error_handle = errors.clone();

        let sender_handle = audio_sender.clone();

        let settings_handle = tracker.settings.clone();

        let preloaded_handle = tracker.preloaded.clone();

        move || {
            let ui = ui_handle.unwrap();

            let snapshot_data = SnapShot::new();

            let settings = settings_handle.read().unwrap();

            if Tracker::read(preloaded_handle.clone()) {
                ()
            } else {
                let file = if settings.recordings.len() > 0 {
                    settings.recordings[ui.get_current_recording() as usize]
                        .name
                        .clone()
                } else {
                    String::new()
                };

                let path = match File::get_directory() {
                    Ok(value) => value,
                    Err(error) => {
                        error.send(&ui);
                        String::new()
                    }
                };

                match sender_handle.send(Message::File(format!("{}/{}.wav", path, file))) {
                    Ok(_) => (),
                    Err(_) => {
                        Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                    }
                }
            }

            match sender_handle.send(if ui.get_input_playback() {
                ui.set_input_recording(false);
                ui.set_audio_playback(false);
                ui.set_input_playback(false);
                ui.set_locked(false);
                Message::StopAudio
            } else {
                ui.set_input_recording(true);
                ui.set_audio_playback(false);
                ui.set_input_playback(false);
                Message::PlayAudio((
                    Playback::Capture(snapshot_data),
                    ui.get_current_recording() as usize,
                ))
            }) {
                Ok(_) => (),
                Err(_) => {
                    Tracker::write(error_handle.clone(), Some(Error::PlaybackError));
                }
            }
        }
    });

    // Update UI when playing is finished
    ui.on_sync_playing_with_backend({
        let ui_handle = ui.as_weak();

        let finished = tracker.playing.clone();

        let sender_handle = audio_sender.clone();

        let settings_handle = tracker.settings.clone();

        let error_handle = errors.clone();

        move || {
            let ui = ui_handle.unwrap();

            if Tracker::read(finished.clone()) {
                // If finished playing
                let settings = settings_handle.read().unwrap();

                if ui.get_playback() == PlaybackType::None {
                    // If playback type is set to stop playing at the end of the song
                    // Update UI and do nothing
                    ui.set_input_recording(false);
                    ui.set_audio_playback(false);
                    ui.set_input_playback(false);
                } else if ui.get_playback() == PlaybackType::Loop
                    || ui.get_playback() == PlaybackType::AutoNext
                // If looping or auto skippng to next song
                {
                    match sender_handle.send(if ui.get_input_recording() {
                        // Stop audio if recording inputs
                        ui.set_input_recording(false);
                        ui.set_audio_playback(false);
                        ui.set_input_playback(false);
                        Message::StopAudio
                    } else {
                        if ui.get_playback() == PlaybackType::AutoNext {
                            // If auto skipping
                            let settings = settings_handle.read().unwrap();
                            // Skips to first recording if on last recording, otherwise skips to next recording
                            // Also handles shuffle logic
                            if ui.get_shuffle() && settings.get_index_data().recording_length > 2 {
                                if ui.get_current_shuffle_index()
                                    == (ui.get_shuffle_order().row_count() - 1) as i32
                                {
                                    // If on last index in shuffle list, reshuffle and set index to 0
                                    ui.invoke_gen_shuffle();
                                    ui.set_current_shuffle_index(0);
                                } else {
                                    ui.set_current_shuffle_index(
                                        ui.get_current_shuffle_index() + 1,
                                    ); // Otherwise increase shuffle index by one
                                }
                                ui.set_current_recording(
                                    ui.get_shuffle_order()
                                        .row_data(ui.get_current_shuffle_index() as usize)
                                        .unwrap(),
                                ); // Set current recording to shuffle index
                            } else {
                                if ui.get_current_recording()
                                    == (settings.recordings.len() - 1) as i32
                                {
                                    ui.set_current_recording(0);
                                } else {
                                    ui.set_current_recording(ui.get_current_recording() + 1);
                                }
                            }
                            ui.set_current_dial_values(ModelRc::new(VecModel::from(
                                settings.recordings[ui.get_current_recording() as usize]
                                    .parse_vec_from_recording(),
                            )));
                            ui.invoke_skip_audio(); // Invokes skip callback
                        }
                        let snapshot_data = match load(
                            // Load snapshot data
                            &settings.recordings[ui.get_current_recording() as usize].name,
                            LoadType::Snapshot,
                        ) {
                            Ok(DataType::SnapShot(data)) => data,
                            _ => {
                                Error::LoadError.send(&ui);
                                SnapShot::new()
                            }
                        };
                        Message::PlayAudio((
                            // Send the correct play message to UI depending on what button has been pressed
                            if ui.get_audio_playback() {
                                Playback::Generic(snapshot_data)
                            } else if ui.get_input_playback() {
                                Playback::Input(snapshot_data)
                            } else {
                                Playback::Generic(snapshot_data)
                            },
                            ui.get_current_recording() as usize,
                        ))
                    }) {
                        Ok(_) => (),
                        Err(_) => {
                            Tracker::write(error_handle.clone(), Some(Error::MessageError));
                        }
                    }
                }
                Tracker::write(finished.clone(), false);
            }
        }
    });

    // Update dial values when playing back inputs
    ui.on_snapshot_dial_update({
        let ui_handle = ui.as_weak();

        let dials = tracker.snapshot_frame_values.clone();

        move || {
            let ui = ui_handle.unwrap();

            let dial_values = dials.read().unwrap();

            ui.set_current_dial_values(ModelRc::new(VecModel::from(
                Recording::parse_vec_from_list(*dial_values),
            )));
        }
    });

    // Check for any errors and update UI
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
                            // Reload audio if incorrect mesaage sent to thread
                            // This ensures that it won't keep failing
                            if ui.get_audio_or_input_playback() || ui.get_input_recording() {
                                let settings = settings_handle.read().unwrap();

                                let file = if settings.recordings.len() > 0 {
                                    settings.recordings[ui.get_current_recording() as usize]
                                        .name
                                        .clone()
                                } else {
                                    String::new()
                                };

                                let path = match File::get_directory() {
                                    Ok(value) => value,
                                    Err(error) => {
                                        error.send(&ui);
                                        String::new()
                                    }
                                };
                                match sender.send(Message::File(format!("{}/{}.wav", path, file))) {
                                    Ok(_) => (),
                                    Err(_) => (),
                                }
                            }
                        }
                        _ => (),
                    }
                    // Sets all playback UI variables to false and sends error to UI
                    ui.set_recording(false);
                    ui.set_audio_playback(false);
                    ui.set_input_playback(false);
                    ui.set_input_recording(false);
                    error.send(&ui);
                    Tracker::write(error_handle.clone(), None);
                }
                None => (),
            }
        }
    });

    // Generates a shuffle list and sends it to the UI
    ui.on_gen_shuffle({
        let ui_handle = ui.as_weak();

        let settings_ref_count = tracker.settings.clone();

        move || {
            let ui = ui_handle.unwrap();

            let settings = settings_ref_count.read().unwrap();

            if ui.get_shuffle() {
                if settings.recordings.len() > 2 {
                    ui.set_shuffle_order(ModelRc::new(VecModel::from(Recording::shuffle(
                        settings.recordings.len(),
                    ))));
                } else {
                    Error::ShuffleError.send(&ui);
                }
            }
        }
    });

    ui.run()?; // Runs UI

    Ok(()) // Returns Ok if Ok
}
